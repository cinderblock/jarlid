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
- Scratch: `%LOCALAPPDATA%\Temp\dopamine-fr-en`.

## Decisions already made (don't re-ask)

- No app change for this song. A one-off script, plus the app's existing editor for the review.
- Nothing is published without Cameron's review.

## Plan / steps

1. [x] Hear the song twice: whole, and each grid line as its own clip. CPU, 5 min.
2. [x] Score each line against both languages, and mark what can't be told.
3. [x] Install the mix as a half-reviewed draft: a local edit, in the app's lyrics-overrides.
4. [ ] **Cameron reviews it in the app's editor.** ← current
       Listened 2026-11-02: mostly right, a couple of picks corrected, the outro settled by ear
       (English then French). 65 of 67 ticked. Left: the two `oo-oo-oo` lines at 0:35 and 1:32,
       which Cameron holds until choosing a spelling.
5. [ ] Publish from the editor. It files under record 2785399's own name, so it replaces that
       French-only record rather than adding one beside it. Cameron's call.

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
- **The vocal at 0:35 and 1:32, across all 22 records** (2026-11-02): `Ooh-ooh-ooh-ooh-ooh` 12,
  `Ooh-ooh-ooh-ooh` 1, `Ooh-ooh-ooh-ooh (Mmh-mmh-mmh-mmh)` 2, `Hum, hum, hum` 4, no line at all 3.
  The Hum records are French or French + English; every English and Split Brain record has Ooh.
  Records saved in `%LOCALAPPDATA%\Temp\dopamine-fr-en\all-records.json`.
- 11043109's English at 1:25 repeats "And it's nice, but I wonder" against "Bien perdu dans le
  confort", which looks like an upload mistake. The grid has "Now I'm lost in the comfort" until a
  real English record can be checked.
