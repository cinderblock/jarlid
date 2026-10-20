# Jarlid

**Jarlid** (the lid of the jar — in the original myth, Pandora opened a *pithos*) is an
unofficial desktop client for a **paid Pandora account**, built after Pandora discontinued their
official desktop app. It is a **reimplementation of Pandora's client protocol**: it speaks
Pandora's own APIs directly, decodes and plays the audio itself, and puts the current song's
**album art and synced lyrics front and center**. There is no browser and no embedded web player
anywhere in it.

## Features

- **Synced (karaoke-style) lyrics** from [LRCLIB](https://lrclib.net), with version matching by
  title, album and duration. When it picks the wrong version, the versions button lists every
  record LRCLIB found and lets you try each one live against the song, then keep the right one
  for that track. Records that carry every line in two languages (a translation, or a bilingual
  version) show both, the second as a subtitle, with a chip to swap them or show one.
  Also an on-disk cache (lyrics are rechecked daily in the background, and a miss
  is remembered for two hours), and per-track sync nudging (`[` / `]`, ±0.25 s).
  A lookup that cannot reach LRCLIB is retried while the song plays, and is never reported as
  "no lyrics found".
  Lyrics with no timestamps still follow the song: the pane drifts through them in proportion to
  the elapsed time, and holds still while you scroll it yourself.
- **Auto-synced lyrics** *(opt-in)*: when LRCLIB has a song's words but not their timings,
  Jarlid works them out on your computer. A speech model
  ([Parakeet v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3), run by whisper.cpp) hears
  the song, and the known words are lined up against what it heard. Only timings are added: the
  words, and their order, are LRCLIB's exactly, however the model misheard them. Lines it
  didn't hear at all are placed from the song's line pace and its stanzas. The next song is
  worked on while the current one plays, so it is usually ready when it starts; a song that
  starts with nothing prepared fills in from the playhead as it is heard. Across 56 test songs,
  counting every line shown, 74% of lines land within 0.3 s of LRCLIB's hand-synced timings, 90%
  within 1 s and 96% within 3 s, on songs where the model heard enough of the vocals to pass
  the quality check. The engine is about 700 MB and is
  downloaded only when you turn this on in Settings; every file is checked against a hash this
  build was released with. Nothing is published on its own: auto-synced lyrics are marked as
  such, and once you've watched them line up, **Looks right? Publish** beside the lyrics (or the
  timing editor, to fix them first) sends them to LRCLIB after asking.
- **Flag lyrics as wrong**: press `F` while a song plays to flag the line being sung, without
  stopping anything (the line before comes too if this one has only just started). The flag
  button above the lyrics opens flag mode: click a line to flag it, shift-click to flag a
  stretch, flag the whole song, and say what's wrong (timing, words, wrong version, a note).
  Flagged lines get a wavy underline. Pandora won't replay a song on request, so Settings lists
  every flagged song. Flags stay on your computer, and each keeps a copy of the lyrics it was
  made on. For auto-synced lyrics it also keeps what the speech engine heard, so the song can
  be re-timed later without hearing it again.
- **Native Windows media integration**: hardware media keys and the volume-flyout / lock-screen
  media panel (title, artist, album art, live state) via a real SMTC session, fed from the player
  itself rather than inferred from a page. Plus a **taskbar thumbnail toolbar** (thumbs down ·
  replay · play/pause · skip · thumbs up) under the taskbar hover preview, drawn with the same
  artwork as the in-app transport and following the system light/dark theme.
- **Transport & stations**: play/pause (Space), skip, thumbs, replay, searchable station
  picker, recently-played art gallery with per-track detail modal.
- **Station Modes**: switch a station between My Station, Crowd Faves, Discovery, Deep Cuts,
  Newly Released, Artist Only, Energy Boost and Relax, with Pandora's own description of each
  shown inline. The mode is part of the station button in the top bar,
  `STATION NAME | Crowd Faves ▾`, because it qualifies the station rather than being a place
  of its own. Switching clears the queued tracks so the change is audible within a song or two.
- **QuickMix shows its source**: QuickMix blends dozens of your stations, and each track says
  which one it came from — otherwise there's no way to tell what you're actually listening to.
- **Network player (UPnP/DLNA + WiiM) remote mode**: a **This computer | speaker** toggle in
  the top bar, always on screen once a renderer is found, picks which source the screen, the
  play button (and Space) and the media keys drive; a dot marks whichever is audibly playing.
  On a narrow window the toggle gives way after the station name: " · idle" goes, then the
  speaker's name shortens, then it shows an icon per side, with the words in a tooltip. Whichever
  source *starts* playing takes over on its own (the speaker only while local is idle), and
  pausing never moves it — so Play resumes whatever you paused. With the speaker chosen,
  Jarlid is its display: art, synced lyrics, and (on a WiiM) the Pandora station it is on,
  marked in the Stations list. The source *not* on screen shows as a **mini player** while it
  plays, with its own play/pause; click it to switch. WiiM devices use the native LinkPlay
  API, so metadata works for the WiiM's own sources too,
  and every row on the Stations page gets a cast button that starts that station directly on
  the device over its PlayQueue service (any station, not only saved presets), plus
  play/pause/skip control.
- **Native audio**: 128 kbit/s MP3 (double Pandora's 64 kbit/s default), decoded and resampled to
  the output device's rate, streamed with a lock-free ring buffer. Playback position is measured
  from frames actually delivered to the device, which is what keeps synced lyrics honest — decoding
  runs several seconds ahead of what you hear.
- **Holds up on a busy machine**: both audio threads register with Windows' multimedia scheduler
  (MMCSS) — the decoder as `Audio`, and the output callback as `Pro Audio`, because the audio
  library it runs on (cpal 0.16) means to raise that thread itself but passes a thread id where
  a handle belongs, and leaves it at normal priority. The debug log is how that was found: every
  underrun was the callback waiting 30–40 ms for a CPU, with the callback itself taking a tenth
  of a millisecond. And the device buffer adapts. It starts at 30 ms, too short to notice on a pause, and when the
  machine is busy enough for the sound card to run dry twice in a minute, it doubles, up to
  240 ms. After ten quiet minutes it shrinks back. Growing mid-song reopens only the stream to
  the device, keeping the decoder and the seconds of audio already decoded, so it costs a few
  tens of milliseconds rather than a reconnect; the same move follows the default output when
  Windows changes it. The playhead accounts for audio still waiting in that buffer, so synced
  lyrics stay on time at any size, and the end of a song plays out of it before the next one
  starts rather than being cut off with it. The current size is in the technical readout. Both
  kinds of dropout are counted there and in the issue report: silence from decoding falling
  behind, and underruns from the output being late. With the debug log on, each underrun is
  logged with how long the output callback waited before it, which tells whether the callback
  was late or the device had nothing left anyway.
- **Saved songs (opt-in)**: Pandora repeats songs, and each repeat would otherwise be downloaded
  again. Turn this on and pick a size in Settings (each size shows roughly how many songs it
  holds). The next song is fetched whole while the current one plays and saved, so a first play
  uses no more data than streaming would, and a repeat plays from disk. The songs played longest
  ago make room when it fills. Adverts are never saved. Off by default, because Pandora's terms
  don't generally allow keeping their audio outside their own offline feature.
- **Playback that survives being left alone**: a long pause releases the stream and the audio
  device rather than sitting on a connection that the far end will quietly drop, and pressing play
  re-opens the track and seeks back to the second you paused on. The same watchdog covers a stream
  that stalls mid-song or an output device that disappears (headphones unplugged, monitor asleep):
  it rebuilds where you were instead of going silent until you hit skip. Because it watches the
  *decoder* rather than the playhead, a stalled read is caught with the whole ring buffer still in
  hand — the re-open usually finishes before there is anything to hear. Pause for long enough that
  Pandora's signed stream URLs have expired and play moves straight on to a fresh song — it does
  not try the stale ones queued behind it one by one, and an advance it makes on its own never
  overrides a pause you just pressed.
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
- **Its own volume**: a level for Jarlid alone, separate from the Windows one, so the radio
  can sit *under* the rest of the machine — turn Windows up and notifications, calls and
  video land above the music instead of competing with it. It is a constant-dB fader over a
  60 dB range, so every 1% of travel is the same 0.6 dB and the slider feels identical
  wherever you grab it — a power law, by contrast, is imperceptible at the top and twitchy at
  the bottom. The gain is applied in floating point before the samples are quantised, so
  turning down costs no resolution at all, and every change is ramped over ~15 ms so a
  dragged slider is never heard as a click. Windows' per-app mixer can do something similar,
  but it is three dialogs away, invisible from here, and forgets itself when the output
  device changes.
- **Output device**: play on whatever Windows currently calls the default *and keep following
  it* — change the default mid-song and playback moves within a second, rather than carrying
  on to the speakers you just switched away from. Or pin Jarlid to one device and leave it
  there. A chosen device that gets unplugged falls back to the default without forgetting the
  choice, so plugging it back in restores it.
- **Technical readout, including the song's BPM**: a faint line in the bottom-left corner,
  opposite the version badge — tempo, what is actually being decoded, and whether it is being
  decoded well. **Pandora sends no tempo of any kind**, so the BPM is *measured*, from the same
  samples that reach the speakers: a three-band onset envelope, autocorrelated and comb-filtered,
  with no FFT and no new dependency. It takes about ten seconds to appear, because decoding is
  deliberately throttled to roughly playback speed and there is genuinely no more to go on. A
  reading it isn't sure of is marked `~`, and music with no steady pulse — spoken word, rubato
  piano — gets no number rather than an invented one. The codec and bitrate beside it are read
  from the container rather than from Pandora's `audioEncoding` label, which describes the
  stream we *didn't* ask for; where the source and device rates differ, both are shown, because
  that gap is the resampling that keeps pitch honest. Then the two buffers: decoded audio in
  hand (`buf 4.9 s`) and the device buffer (`out 30 ms`), which shows `out 30 → 60 ms` when it
  has just grown under load and the next song (or a reopen, if the underruns go on) will pick
  that up. The three fault counters —
  audio lost, audio that arrived too late to play, and device underruns — appear only when they
  are non-zero, so a line that stays quiet means what it says.
- **Blending between songs** *(experimental, off by default)*: overlap the end of one track with
  the start of the next, and — in Beat-matched mode — pull the incoming song's tempo onto the
  outgoing one so the beats land together. The pull is a **pitch fader**, exactly as a DJ's is:
  speed and pitch move together, and the limit is set as a percentage because ±6% is the same
  musical stretch at 90 BPM as at 160. That limit doubles as the decision to blend at all — two
  songs further apart than it allows get an ordinary transition, because no blend beats a bad
  one. Half and double time count as already matched, so a 64 BPM track lines up with a 128 BPM
  one without either being touched.

  It works by pulling the next track's opening into memory ahead of time — decoding is about 120×
  realtime when nothing throttles it, so that costs well under a second — measuring its tempo
  there, and then, when the fade finishes, re-pointing the live stream underneath the audio that
  is already playing rather than opening a new one. That last part is what keeps it gapless: a
  fresh stream would take a couple of hundred milliseconds to open and seek, which is unnoticeable
  between songs and very noticeable in the middle of one.

  **Known issue:** the window can go blank after a blend hands over. That is why it ships off.
- **Settings**: light, dark, or follow Windows (the window frame follows too, and "system"
  keeps following when Windows switches at sunset); Jarlid's own volume; which account is
  signed in and a way to sign out (which clears the saved password from the Windows
  Credential Manager); and how updates should arrive. Every control on the page is drawn by
  the app rather than by the platform, so the dropdowns, radios and sliders belong to the
  same UI as everything else.
- **Debug log (opt-in)**: a switch under Troubleshooting in Settings writes everything Jarlid
  logs to `%LOCALAPPDATA%\com.camer.pandora-desktop\logs\jarlid.log`, stamped with the local
  time to the millisecond. That covers each song starting and ending, the audio device and buffer
  size opened, every device underrun and starved stretch with where in the song it happened, the
  buffer growing, stalls being recovered, errors and panics. Off by default; once on it stays on
  across restarts and updates. A release build has no console, so without it all of that is
  thrown away. It captures stderr itself, so nothing that logs needs to know about it. Capped at
  two 8 MB files.
- **Updates that never interrupt anything**: the installer is downloaded and
  signature-verified in the background, then installed at a moment that costs you nothing.
  If the music is paused it goes in immediately and silently, and Jarlid comes back *still
  paused* — you would only know from the version number. Signed out, sitting on the login
  card, there is nothing to protect at all, so it just gets on with it and comes back ready
  to sign in. Otherwise it waits for the gap
  *after* a track rather than cutting through the middle of one, restarts, and picks the
  station back up, so an update costs a slightly longer pause between songs. You choose the
  behaviour — install after the current song (default), install immediately, download but
  ask first, or just be told a version exists — and how often to look, from every 30 minutes
  to once a day at a wall-clock hour you are not listening at. It will not restart during a
  station export. If a network player is doing the playing, that is the easiest moment of all
  — Jarlid only watches a WiiM rather than streaming to it, so the speaker plays straight
  through the restart; Jarlid comes back quiet so it never starts a second stream over the top.
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
  resampling to the output device's rate; a lock-free ring buffer feeds cpal. Tempo is measured
  on the decode thread as the samples go past — it is the one thread that sees every sample in
  order and is allowed to think, unlike the output callback, where a lock once caused continuous
  scratchiness.
- **`crates/engine`** — the radio. Queue refill, auto-advance, transport, thumbs, station Modes,
  the adaptive device-buffer policy, the opt-in song cache (`cache.rs`, keyed by `pandora_id`),
  and credentials in the Windows Credential Manager.

Auto-synced lyrics add two more, kept apart so the large speech engine is never in the installer:

- **`crates/autosync`** — pure Rust. Lines up a lyric's words with timed words a speech model
  heard (a sequence alignment over normalised words, in which lyric words the model missed still
  need time to be sung), cuts a song into the overlapping windows the model is fed, and scores
  results against known-good synced lyrics. Its `bench` example and `engine`'s `autosync-corpus`
  example are the measurement harness behind every number in `plans/auto-synced-lyrics.md` and
  `plans/autosync-lyrics-first.md`.
- **`crates/autosync-sidecar`** — `jarlid-autosync.exe`, the downloadable sidecar. It loads
  whisper.cpp's Parakeet runtime from DLLs at run time and answers a JSON-lines protocol on stdio
  with timed words, a window at a time. `build-engine.ps1` builds the DLLs from a pinned
  whisper.cpp release, with every CPU instruction-set level and Vulkan selected at run time;
  `package.ps1` writes the manifest of file hashes; an `autosync-v*` tag publishes it all.
  `app/src-tauri/src/autosync/` downloads and verifies it, runs it at below-normal priority, and
  does the aligning, so aligner fixes ship with ordinary app updates.

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

Frontend unit tests: `bun run test` in `app/`. Rust: `cargo test` in `app/src-tauri/` and in
`crates/`.

Auto-synced lyrics are offered only by builds that pin an engine release (`MANIFEST_SHA256` in
`app/src-tauri/src/autosync/component.rs`). To try them in a dev build before that, build the
engine and sidecar locally and point the app at a manifest of local files:

```sh
pwsh crates/autosync-sidecar/build-engine.ps1 -Out C:/tmp/engine
cargo build --release -p autosync-sidecar --manifest-path crates/Cargo.toml
cp crates/target/release/jarlid-autosync.exe C:/tmp/engine/
pwsh crates/autosync-sidecar/package.ps1 -Dist C:/tmp/engine -BaseUrl file:///C:/tmp/engine
JARLID_AUTOSYNC_MANIFEST=C:/tmp/engine/autosync-manifest.json bun run tauri dev
```

Requirements: Rust, Bun, and the WebView2 runtime (WebView2 renders Jarlid's own UI; it no longer
loads pandora.com). First launch: sign in with your Pandora account — the password goes to the
Windows Credential Manager, encrypted for your user.

**Build with the Tauri CLI, not bare `cargo build`.** Only the CLI embeds the built frontend; a
plain `cargo build --release` produces a binary that tries to load the dev server and fails with
`ERR_CONNECTION_REFUSED`.

In a dev build the version bar bottom-right names the commit it is really running, because the
version number alone only identifies a *release*: `v1.5.3 · master@f15dc14`, with a trailing `*`
when the tree is dirty. Sitting clean on the matching `vX.Y.Z` tag it shows just the version, as
a release does. Updates are disabled in a dev build — the background loop is compiled out and
the bar will not install one — so **clicking it copies the full commit sha** instead, which is
the part you cannot read off the short hash on screen. That works on the tag too, where the bar
is showing only the version.

`scripts/stress-window-move.ps1` is a regression test for the window-state saver deadlock fixed
in v0.6.12 — it hammers the window with moves timed to collide with the debounced state save.
Worth running against a release build after touching window-state or event-loop code:

```sh
pwsh scripts/stress-window-move.ps1 -Exe app/src-tauri/target/release/jarlid.exe
```

The app icon, a glass mason jar with its lid popped off and a note breaking free, is drawn by
`app/src-tauri/icons/source/jarlid-art.py`. It models the jar in 3D and writes two SVGs:
`jarlid.svg` for 40 px and up, and `jarlid-small.svg`, a heavier version for the 16–32 px sizes
the taskbar actually shows. Don't edit the SVGs or the PNG/ICO/ICNS files; change the script and
regenerate the whole set (needs Pillow):

```sh
python scripts/build-icons.py
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
