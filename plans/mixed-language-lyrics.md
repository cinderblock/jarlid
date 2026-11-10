# Lyrics for a song that alternates languages, line by line

## Goal

Madelline, "dopamine (French + English Version)": verses go back and forth between the French
and English recordings. LRCLIB has 22 records for the song and none alternate; each is French,
English, or both at once. Cameron, 2026-10-30: the timing is right, no custom app version for
one song, just a one-off tool to validate one language or the other on every verse.

## Environment / context

- Track key `madelline|dopamine (french + english version)|dopamine (french + english version)`
  hashes to `545b7f6f3922d6ba`, confirmed against the cache file of that name. A local edit goes
  in `%APPDATA%\com.camer.pandora-desktop\lyrics-overrides\545b7f6f3922d6ba.json`.
- Audio: `%LOCALAPPDATA%\com.camer.pandora-desktop\audio\TR_91277714.mp3` (165.5 s).
- Records: 2785399 (this track, French only, 55 lines, the timing grid); 11043109 (Split Brain,
  each line in both languages, coarser lines, about 0.25 s later).
- Engine: Parakeet v3 q8 through the shipped `jarlid-autosync.exe`; CPU, about 100 s for the song.
- Scratch was `%LOCALAPPDATA%\Temp\dopamine-fr-en` and `...\Temp\dopamine-pub`; both were
  wiped 2026-11-09 (see Findings). What matters now lives in the repo:
  `scripts/mix-lyrics-dopamine-published.json` (the published text, as the app's override JSON)
  and `scripts/check-dopamine-lrclib.py`.

## Decisions already made (don't re-ask)

- No app change for this song. A one-off script, plus the app's existing editor for the review.
- Nothing is published without Cameron's review.

## Plan / steps

1. [x] Hear the song twice: whole, and each grid line as its own clip. CPU, 5 min.
2. [x] Score each line against both languages, and mark what can't be told.
3. [x] Install the mix as a half-reviewed draft: a local edit, in the app's lyrics-overrides.
4. [x] Cameron reviewed it in the app's editor, 2026-11-02: a couple of picks corrected, the outro
       settled by ear (English then French). Then by Cameron's choice: `Ooh-ooh-ooh-ooh-ooh` at
       0:35 and 1:32, the "ah" at 0:54.86 kept, `My D-O-P-A-M-I-N-E` and `Ma D.O.P.A.M.I.N.E`.
       Those were written straight into the override, which also cleared its `draft`, so it is
       now an ordinary local edit. The final text lives there, not in the script's output: a
       rerun would lose Cameron's corrections. Kept in the repo since 2026-11-09:
       `scripts/mix-lyrics-dopamine-published.json`.
5. [x] Published 2026-11-02, at Cameron's request, by script rather than the editor: the same
       challenge and `POST /api/publish` as `publish_lyrics`, under record 2785399's own name.
       201, and `/api/get/2785399` and the track lookup both serve all 67 lines unchanged. As the
       editor does after a publish, the override and the old cache entry were cleared (moved to
       `%LOCALAPPDATA%\Temp\dopamine-pub\`). Only 2785399 changed; the Split Brain, English and
       French records were not touched. LRCLIB keeps the previous revision.
6. [x] First play, 2026-11-05 4:02 PM PST (00:02Z): **the old French was back.** LRCLIB itself
       serves it again (`/api/get/2785399` and the track lookup, `cf-cache-status: DYNAMIC`, so
       not a CDN cache), 55 lines, with a matching `lyricsfile`. Our 67 lines are not on any of
       the 20+ records a search returns. See Findings.
7. [x] Restored the mix locally, 2026-11-05: `dopamine-pub\override-published.json` copied
       back to `lyrics-overrides\545b7f6f3922d6ba.json` (copy kept). The app shows it as a local
       edit ("edited" label) no matter what LRCLIB serves.
8. [x] Republished 2026-11-05 4:48 PM PST (00:48Z), at Cameron's yes: 201, both read-backs
       match. The local edit stays in place as a safety net.
9. [ ] Watching every 12 h (T3 scheduled task "Watch dopamine lyrics on LRCLIB", this thread):
       `python -I scripts/check-dopamine-lrclib.py`, logging to
       `%LOCALAPPDATA%\com.camer.pandora-desktop\dopamine-watch\check.log`. Still ours at every
       check from 2026-11-05 4:48 PM PST (00:48Z) on. A revert is reported, never republished
       without asking. No end date set. ← current

To regenerate (`--hear` only if the grid's times change):

    python -I scripts/mix-lyrics.py scripts/mix-lyrics-dopamine.tsv <16 kHz mono wav> <jarlid-autosync.exe> <model.bin> <out> --hear --record <lyrics cache file>

Undo: the editor's clear-edit, or delete the override file; the song goes back to record 2785399.

## Result

67 lines: 30 French, 25 English, 12 Da-ra-da. 21 marked for an ear, all in four places:

- **Intro, 0:00-0:05 (4 lines).** Picked French. Scores were weak, but with the song's first 8 s
  heard alone, all four channels came back with the same sounds, "to it, too scarce, book,
  shovel, shovel": *tout ce que je veux, je veux, je veux*.
- **Hum / Ooh, 0:35 and 1:32 (2).** Nothing heard. Picked Hum, the record's text.
- **Ma / My D.O.P.A.M.I.N.E, 1:01-2:12 (6).** Differ by one word, so each takes the language of
  the line before it: Ma through 1:16, My from 1:58.
- **Tu m'contrôles, 1:37 (1).** Heard "Tu moet troll of true pool": French, just under the bar.
- **Outro, 2:14-2:26 (8).** Unresolved. Picked French. See Findings.

Not marked but worth a look: the 12 Da-ra-da lines aren't in record 2785399. They come from
11043109 with its times moved 0.25 s earlier, so their timing is an estimate, not the record's.

## Findings / gotchas

- **`%LOCALAPPDATA%\Temp` gets wiped.** Between 4:48 AM and 3:41 PM PST 2026-11-09, both
  scratch folders lost everything but `check.py` and `check.log`: the reference copy,
  `publish.py`, the audio, `heard.json`, `all-records.json`. The 4:48 PM check then failed with
  `FileNotFoundError: ...dopamine-pub\override-published.json`, not a revert (LRCLIB still
  matched). Keep anything that must last in the repo or the app's data folder, not Temp.

- **The 2026-11-02 publish didn't stick.** By 11-05 the record was the old French again. Not the
  missing `lyricsfile` in our request: LRCLIB builds one from the LRC (`publish_lyrics.rs`,
  test `legacy_publish_generates_and_persists_lyricsfile`). Not its backfill jobs either: per its
  `ARCHITECTURE.md` they never move `tracks.last_lyrics_id`. Left: someone republished the old
  French (a tool that re-uploads, say), or a server restore. The API shows no revision history
  or dates, so which one is unknown. A republish could be undone the same way.

- **Outro: the engine can't say.** Whole-song pass: nothing. Line clips: nonsense ("I like you, I
  like the me"). Left, right and side channels disagree, with "I want", "I need" and "should be"
  in them, which fits both versions layered. Could be either, or both at once.
- Scoring needs a floor as well as a gap: 0.46 vs 0.32 passed the gap alone and was a coin toss.
- **Reuse, not a new UI:** an override with `draft` set is exactly a half-reviewed saved draft, so
  the editor's ✓ per line, underlines and Publish lock work with no app change. Checked against
  the real `applyReview` / `reviewState`: 67 to review, 21 marked, all ticked gives `null`.
- **The engine already switches language by itself.** One pass over the song heard "You're keeping
  me awake", "C'est bien chouette mais j'ai pensé", "Je te sens jusqu'au bout de mes doigts" and
  "You're everything I want" without being told anything. Nothing in `crates/` sets a language.
- It writes sung French hooks as English nonsense ("two squash your book" for "tout ce que je
  veux"), and the whole-song pass heard nothing after about 2:00.
- **LRCLIB wasn't down for the whole first session, as first reported.** It did answer 503
  ServerOverloaded, and still does intermittently, but the later `000`s were this machine's curl
  failing (`curl: (43) A libcurl function was given a bad argument`), not timeouts. Python's
  `urllib` with retries reads it fine (2026-11-02). Use that, not curl.
- **Spelling across the 22 records:** `My D-O-P-A-M-I-N-E` in 15, `My D.O.P.A.M.I.N.E` in none;
  `Ma D-O-P-A-M-I-N-E` in 10, `Ma D.O.P.A.M.I.N.E` in 4. The dotted form is French only, but
  hyphens are the more common French spelling too.
- **The vocal at 0:35 and 1:32, across all 22 records** (2026-11-02): `Ooh-ooh-ooh-ooh-ooh` 12,
  `Ooh-ooh-ooh-ooh` 1, `Ooh-ooh-ooh-ooh (Mmh-mmh-mmh-mmh)` 2, `Hum, hum, hum` 4, no line at all 3.
  The Hum records are French or French + English; every English and Split Brain record has Ooh.
  Records saved in `%LOCALAPPDATA%\Temp\dopamine-fr-en\all-records.json`.
- 11043109's English at 1:25 repeats "And it's nice, but I wonder" against "Bien perdu dans le
  confort", which looks like an upload mistake. The grid has "Now I'm lost in the comfort" until a
  real English record can be checked.
