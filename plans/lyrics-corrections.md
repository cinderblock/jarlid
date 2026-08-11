# Lyrics corrections — editing what LRCLIB has

## Goal

When a lyric line is wrong, give the listener a subtle, quick way to fix it — ideally
correcting the shared LRCLIB database, not just papering over it locally.

Triggering case (2026-08-11): Klaas — "Better Off Alone" renders a line as
"Better all so long". Verified this is a genuine typo in the LRCLIB record, not a bad
match: record id `20042741`, line `[00:59.99] Better all so long` (should be
"Better off alone"). The rest of the entry is correct for the track.

## Environment / context

- App: Jarlid (this repo), Tauri v2. Lyrics fetched in `app/src-tauri/src/lib.rs`
  (`fetch_lyrics` command) from LRCLIB, disk-cached under the app cache dir at
  `lyrics/` keyed by FNV-1a of `artist|track|album`. **Misses are not cached.**
- UI: `app/src/main.ts` (`loadLyricsFor`, `renderSyncedLyrics`, `parseLrc`),
  markup in `app/index.html` (`section.lyrics-pane` → `#lyrics-status`, `#lyrics`),
  styles in `app/src/styles.css` (~line 720).
- The `Lyrics` struct currently carries only `synced`, `plain`, `source` — the matched
  LRCLIB record's `id` and canonical metadata are **discarded**. Both are needed to
  correct anything (see "Findings").
- `tauri-plugin-opener` is already a dependency and `opener:default` is already granted
  in `app/src-tauri/capabilities/default.json`. Rust-side usage example:
  `diagnostics.rs:318`.
- Custom tooltips via `attachTip` in `main.ts:250`. No native `title=` anywhere.

## Findings — what LRCLIB actually supports for corrections

Pulled from the docs at <https://lrclib.net/docs> (client-rendered; text extracted from
the site JS bundle) and confirmed against the live API.

**Corrections are possible. Two endpoints, both anonymous, no registration.**

### 1. `POST /api/publish` — the real edit path

- Publishing for a track that **already has lyrics** is how you correct it. Docs:
  > "All previous revisions of the lyrics will still be kept when publishing lyrics for
  > a track that already has existing lyrics."

  So it is revision-based: the new publish becomes current, history is retained. There
  is no `DELETE` and no in-place `PATCH`.
- Body (all four metadata fields **required**): `trackName`, `artistName`, `albumName`,
  `duration`; then optional `plainLyrics`, `syncedLyrics`, `lyricsfile`.
- **The four metadata fields are the identity of the record.** To correct an existing
  entry rather than create a sibling, they must match the target record exactly — which
  means using the record's *own* strings, not Pandora's now-playing metadata.
- All three lyrics fields empty ⇒ the track is marked **instrumental**.
- `lyricsfile` (raw Lyricsfile YAML) takes precedence and causes `plainLyrics` /
  `syncedLyrics` to be ignored.
- Success is `201 Created`.

### 2. `POST /api/flag` — report without fixing

- Body: `trackId` (required), `content` (optional free-text reason).
- For "wrong lyrics, wrong track metadata, or a copyright violation". Recorded against
  the track's current lyrics.

### Both require a proof-of-work Publish Token

- Header `X-Publish-Token: {prefix}:{nonce}`. **Single use** — a fresh token per request.
- `POST /api/request-challenge` returns `{prefix, target}`. Live sample:
  `prefix=GyytT7uxw0WjY8o0GZLpA8lgPBt4QnhQ`,
  `target=000000FF00000000000000000000000000000000000000000000000000000000`.
- Solve: find the smallest `nonce` where `SHA256(prefix + nonce)` byte-compares `<=`
  target. Reference implementation (~35 lines):
  `tranxuanthang/lrcget/src-tauri/src/lrclib/challenge_solver.rs`.
- That target means ~2^24 hashes on average ≈ **a few seconds single-threaded in Rust**.
  Cheap enough to do inline on a button press; worth a progress affordance anyway.

### Web UIs that already exist

- `lrclib.net` itself has **no editor**. Its only routes are `/`, `/search/:keyword`,
  `/tracks/:id`, `/docs`, `/db-dumps`, `/lyricsfile` (extracted from its router).
- **LRCLIBup** — <https://lrclibup.boidu.dev> (source: `better-lyrics/lrclibup`) is a
  third-party publish form that does the PoW in a web worker. It **prefills from query
  params**: `?title=&artist=&album=&duration=` (`src/routes/+page.svelte`, reads
  `URLSearchParams`; also accepts `videoId`). There is **no param for the lyrics body**,
  so an external link cannot carry the text to fix — it would have to be pasted.

## Why in-app beats linking out

Three things point the same direction:

1. **No external form can carry the lyrics body.** LRCLIBup prefills metadata only, so
   "fix one typo" via a link means pasting the whole LRC by hand.
2. **Timestamping needs the playhead.** Plenty of LRCLIB records are `plainLyrics`-only.
   Adding sync means tapping along to the song — which only the player can do. A
   tap-to-sync pass turns a plain record into a synced one, published back as a revision
   via the same `POST /api/publish`. This is the single highest-value thing here: it
   fills LRCLIB's biggest gap, and the app is uniquely placed to do it.
3. **The correct publish identity is already in hand** once the record is plumbed
   through — no re-deriving it in a browser form.

The app also already has an `[`/`]` per-track sync-offset nudge (`main.ts`, `syncOffset`,
persisted to `localStorage` as `syncoff:<key>`). A whole-file offset is the degenerate
case of timestamping; a real tap-to-sync pass supersedes it for records worth publishing.

## Decisions already made (don't re-ask)

- The affordance must be **subtle** — this is a now-playing screen, not an editor.
- No native `title=` tooltips; use `attachTip`.

## Scope — settled 2026-08-11

Cameron chose **full in-app editor + publish**, including flagging. Local overrides *and*
upstream publishing, not one or the other.

## Open questions for the user

None outstanding.

## Plan / steps

- [x] **Plumb the matched record through `fetch_lyrics`.** `Lyrics` now carries `id`,
      `trackName`, `artistName`, `albumName`, `duration` and `overridden`, populated in
      `from_lrclib`. `#[serde(rename_all = "camelCase", default)]` — the container-level
      `default` is what lets cache files written before these fields existed still load.
  - **Self-healing cache:** a cache hit with no `id` is treated as a miss, so the
    existing cache refreshes one track at a time instead of needing a wipe or a
    directory rename that would orphan the old files.
- [x] Extract everything lyrics-related out of `lib.rs` into `app/src-tauri/src/lyrics.rs`.
- [x] Local overrides in the **data** dir (`lyrics-overrides/`), not the cache dir — the
      cache is disposable by definition and these are the user's own work. Checked
      before the cache, so an edit applies instantly and offline.
- [x] Proof-of-work solver (`solve_challenge`), threaded across cores.
- [x] `publish_lyrics` and `flag_lyrics` commands.
- [x] Subtle edit affordance: a pencil in a new `.lyrics-head` row beside
      `#lyrics-status`. It is a *sibling* of the status, not a child, because
      `#lyrics-status.flash` animates `transform: scale(1.3)` and would drag the button
      with it.
- [x] `app/src/lyric-editor.ts` — full-page editor, Words + Timing tabs.
- [x] Intro countdown (see below).

## Findings — the pre-first-lyric scroll bug (2026-08-11)

Reported by Cameron: when synced lyrics load, nothing scrolls until the first line hits.

Cause: `highlightLine` computes `idx = -1` while the playhead is before the first
timestamp, and the scroll target was `nodes[idx]` — `nodes[-1]` is `undefined`, so no
scroll happened at all. The pane sat at `scrollTop: 0` (line 1 sitting below the 32vh
top padding) and then jumped to centre when the first line arrived.

Fix: a synthetic `.lyric-intro` row rendered ahead of the lines when the first timestamp
is at least `INTRO_MIN` (2s). It is the scroll target for `idx < 0`, and its inner bar
fills as a countdown to the first line. Three things that are easy to get wrong here:

- The intro row must **not** carry the `.line` class. `highlightLine` indexes the `.line`
  NodeList positionally, so an extra `.line` at the front would offset every highlight
  by one.
- The countdown is updated **before** the `if (idx === activeLineIdx) return` shortcut,
  or the bar would only move when the active line changed — i.e. never, during the intro.
- `activeLineIdx` needed a distinct `NO_LINE = -2` sentinel. It used to be seeded with
  `-1`, which now means the real state "before the first line", so the initial scroll to
  the intro row was skipped by the unchanged-index shortcut.
- It keeps its space when spent (`opacity: 0`, not removed) so the scroll geometry
  doesn't shift under the first line.

## Findings — review pass, before running anything (2026-08-11)

- **The editor goes stale when the song changes.** Editing takes minutes; songs are
  minutes. The words are fine — the editor holds its own copy and files it under the
  track it was opened on — but the *playhead* isn't, so stamping across a track change
  would write the next song's times into this song's file with nothing on screen saying
  so. A track change now calls `notePlaybackMoved()`: drop to Words, disable the Timing
  tab and its transport, and name the track the words still save to.
- A failed fetch left the pencil live while `lastMeta` still described the previous
  track, so an edit would have been filed against the wrong song. It now clears
  `lastLyrics` and hides the pencil.
- Static check worth repeating after any markup change: every `$("...")` in
  `main.ts` and `lyric-editor.ts` is resolved at *import* time, so one wrong id throws
  and blanks the whole app. All 81 currently resolve against `index.html`.

## Things not to do

- Don't key a publish off Pandora's now-playing metadata when a matched LRCLIB record
  exists — mismatched `albumName`/`duration` silently creates a *new* record instead of
  correcting the one being displayed.
- Don't publish with all lyrics fields empty unless the track really is instrumental.
- Don't cache misses (current code already avoids this — keep it that way).
- Don't publish with all lyrics fields empty — LRCLIB reads that as "this track is
  instrumental". `publish_lyrics` refuses it.
- Don't add a `.line` element to the lyrics pane for anything that isn't a lyric line.
- Don't let the editor's transport buttons run in remote (UPnP) mode: `transport` drives
  the *local* engine, so "Restart track" would start playback nobody is listening to.
  Guarded by `canTransport`.

## Progress log

- [x] 2026-08-11 — Confirmed the reported bug is an upstream typo in record `20042741`.
- [x] 2026-08-11 — Established what LRCLIB supports for corrections (publish-as-revision
      + flag, both PoW-gated). Documented above.
- [x] 2026-08-11 — Scope settled: full in-app editor + publish + flag.
- [x] 2026-08-11 — Rust: `lyrics.rs` with fetch/override/publish/flag + PoW. Unit tests
      pass. Measured the real challenge at **2.9s** in a debug build; `[profile.dev.package.sha2]
      opt-level = 3` is what keeps that from being minutes, since `tauri dev` is a debug build.
- [x] 2026-08-11 — UI: pencil affordance, full-page editor, tap-to-sync, intro countdown.
      `bun run build` (tsc + vite) clean.
- [x] 2026-08-11 — Review pass found the stale-editor hazard and two smaller state bugs
      (see Findings). Fixed. Static id cross-check added and passing.
- [x] 2026-08-11 — Committed: `65d396f` (feature + intro countdown), `ef97707` (this
      plan), `440f425` (stale-editor guard).
- [ ] **Not yet done: run the app and drive it end to end.** Nothing here has been
      exercised against a live track — see "Still to verify". This needs Cameron: it
      wants a Pandora login and a playing track, and the app is a native window.

## Still to verify (nothing below has been run in the real app)

1. The pencil appears, is subtle, and opens the editor.
2. Intro countdown: fills, centres, fades at the first line, and the first line lands
   without a jump.
3. Words tab: fix the `Better all so long` typo on the live track, Save, confirm the
   pane updates instantly and the status reads `SYNCED LYRICS · EDITED`.
4. Restart the app; confirm the edit is still there (override file in the data dir).
5. Timing tab: tap a plain-lyrics track through and confirm the stamps land sensibly.
6. Publish: the confirm dialog, the ~3s proof-of-work, a `201`, and then `settle()`
   dropping the local override once LRCLIB serves the same text.
7. Flag: only enabled when there is a record id.
8. Discard my edit: falls back to the LRCLIB copy.
