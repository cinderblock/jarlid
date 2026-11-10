# Lyric sections: Chorus, Verse and Bridge as structure, not lyrics

## Goal

Cameron, 2026-11-10, while ILUKA's "Wings" played: "lyrics are untimed and have things like
'Chorus:' in them. can you add the timings and remove or properly annotate the section labels.
Actually, we should probably have some native support for things like chorus and bridge...."

Two parts:

1. **Wings now:** timed lyrics for ILUKA, "Wings" (LRCLIB record `37935460`, 185 s), with the
   section labels handled properly. This is a local override, never published without Cameron.
2. **Native sections:** Jarlid recognises section labels in any common spelling. It shows them as
   section headings, keeps them out of timing and alignment, and keeps them through edits.

## Environment / context

- Pane LRC parser: `app/src/lrc.ts`. Editor: `app/src/lyric-editor.ts` and `app/src/edit-lines.ts`.
  The editor's save rebuilds `plain` from the timed lines (`lines.map(l => l.text)`, around
  `lyric-editor.ts:712`), so today it **drops** any label that lives only in `plain`.
- Autosync aligns `plain.lines()` against the heard words (`app/src-tauri/src/autosync/mod.rs`
  `run` and `time`). It stores one result per `fnv(plain)`.
- Local overrides: `%APPDATA%\com.camer.pandora-desktop\lyrics-overrides\<fnv key>.json`
  (`Lyrics`: `synced`, `plain`, `source: "local edit"`, record identity, `overridden: true`,
  optional `timing`). Wings' LRCLIB cache file is `%LOCALAPPDATA%\…\lyrics\a19cf2bd9aa2a87a.json`,
  so the override key file is `a19cf2bd9aa2a87a.json` too.
- Wings audio is cached: `%LOCALAPPDATA%\com.camer.pandora-desktop\audio\TR_210167525.mp3`.

## Findings

- **Lyricsfile has no section structure.** The spec says only that "`plain` may include section
  labels, blank lines, annotations, or other text not present in `lines`." Timed `lines` carry
  sung text only. So the right home for a label is the plain text, and timed lyrics get their
  sections by lining the plain text up with the timed lines.
- **Wings' 51% auto-sync was not caused by the labels.** The engine (Oct 20) heard verse 1,
  chorus 1, verse 2 and the bridge, but **no words at all** in chorus 2 (about 95–124 s) or
  chorus 3 (after 145 s). Labels are 7 words out of about 350.
- The LRCLIB text has its own mistakes ("hoar", "Top poppy" vs heard). That is not this task.

## Design

- **Recognising a label** (`sections.ts`, plus a Rust twin for autosync): a whole line that is a
  section name, with an optional number and optional extras. Names: intro, verse, pre-chorus,
  chorus, post-chorus, hook, refrain, bridge, interlude, breakdown, instrumental, solo, outro,
  coda, drop, break, tag. Accepted forms are `Chorus:`, `[Chorus]`, `(Chorus)`, `[Verse 2: Artist]`,
  `Chorus x2`, `CHORUS`, and `Pre chorus`. Anything with more words is a lyric. "Hook me up" is
  a lyric.
- **Canonical spelling** written by Jarlid: `[Chorus]`, Genius-style, which is what most LRCLIB
  plain lyrics already use.
- **Pane:** a label is shown as a small, muted section heading above its first line, not as a
  lyric line, in both synced and plain views. Synced lyrics take their headings from the plain
  text, lined up by order. A label that appears *inside* LRC is shown as a heading too.
- **Autosync:** labels are removed before alignment, because they are never sung.
- **Editor save:** labels in the old plain text are put back into the rebuilt plain text.
- **Publishing:** labels go in `plain` only and never in `synced` or in Lyricsfile `lines`.

## Plan / steps

1. Time Wings offline. Take the heard words for the sections that were heard. Find
   choruses 2 and 3 by matching chorus 1's audio across the track, then copy chorus 1's line
   offsets. Write the override with labels as `[Chorus]` in plain only.
2. Native: the label recogniser, tested in TS and Rust.
3. Pane headings, synced and plain.
4. Autosync strips labels.
5. Editor keeps labels on save.
6. README, then commit.

## Missed repeats in autosync (approved 2026-11-10: "sure")

When the engine hears little or nothing of a block of lines that the lyrics repeat (a chorus),
find that block in the audio by matching it against a copy that *was* heard, and copy the heard
copy's timings across, shifted. Wings is the test case: chorus 2 should land at +56.46 s and
chorus 3 at +112.93 s.

Design:
- **Repeats come from the text, not the labels.** A block is a run of 2 or more consecutive sung
  lines whose normalised text recurs elsewhere in the plain lyrics. Labels are optional, so this
  works on any lyrics.
- **Reference and weak copies.** The reference is the copy with the most words matched (at least
  60%). A weak copy is one with under 30% matched.
- **Search.** The window lies between the last anchored line before the weak copy and the first
  one after it. The template is the reference block's audio features: chroma plus log-band
  spectrum, z-scored per track. The best offset is accepted only if it clearly beats both the
  window's median score and its next-best peak at least 1 s away.
- **Result.** The weak copy's lines get the reference's line times plus the offset, and count as
  anchored. The words they recover count toward coverage, because an audio match is evidence. So
  a song like Wings can pass the 60% gate.
- **Stored.** The offsets found are kept in the stored result (`repeats`), so re-timing from
  stored heard words (an aligner bump) still has them without the audio.
- Pure logic and features live in `crates/autosync` (`repeat.rs`). The app computes features from
  the 16 kHz PCM it already decodes for the engine.

Steps:
1. [x] Python prototype at 16 kHz. Chorus 2 came out at 95.84 s (95.82 by hand) and chorus 3 at
   152.32 s (152.29). A window with no chorus scored 0.13. The acceptance rule is best ≥ 0.5,
   at least 0.15 over the median and at least 0.05 over the next peak.
2. [x] `autosync::repeat` (`candidates`, `Features`, `find`) and `Alignment::anchor_repeats`,
   with synthetic tests. The FFT comes from `rustfft`.
3. [x] App: `run` decodes once, writing the WAV for the engine and computing features from the
   same PCM. `time()` takes `Repeats::Find` or `Repeats::Known`. Stored results keep `repeats`
   and `repeats_checked`. An older result whose lyrics have a missed repeat gets a `Job::Repeats`:
   the audio is decoded and checked, and the engine is not re-run. No `align::VERSION` bump was
   needed.
4. [x] Ignored real-song test (`AUTOSYNC_REAL_SONG`, `repeat::tests::real_song`) on Wings: offsets
   56.49 and 112.95 s, coverage 53% → 88%, 0.8 s for features and search in release mode.
5. [x] Committed as `10db6be`. Released as v1.22.0 (`d4c8efa`) on Cameron's go-ahead,
   2026-11-10. v1.21.0 carried only the section headings.

## Progress log

- [x] Wings override, written 2026-11-10 at about 1:20 PM PST (21:20Z). Chorus 1's audio
  (37–62 s) was matched across the track using spectrum and chroma features, one quarter of
  the chorus at a time. All four quarters agree that chorus 2 starts 56.46 s later and chorus 3
  112.93 s later, to within 10 ms. That fits the words heard on either side (verse 2 ends at
  95.5 s, chorus 2 starts at 95.8 s). "Suck it" (33.40 s) was never heard and is placed between
  "boots" and "throw down". Two text fixes went in where the engine clearly heard otherwise:
  "hoar" became "whore", and "your asking" became "you're asking". The script is in this
  session's scratchpad (`wings.py`, `selfsim.py`) and is not kept.
- [x] Wings per-word timings, added 2026-11-10 (Cameron asked "what about per-word timings?").
  Within each line, the lyric words were matched to the words heard during that line, and
  unheard words were interpolated. Choruses 2 and 3 copy chorus 1's word times, shifted. Two
  lines were fixed by hand. "Who said…" now starts at 30.45 s ("Who" was unheard and had been
  squeezed against "said"). "Won't die for much…" was heard as "or without the mind.
  Spreading…", so its words are spread over those syllables. All 48 sung lines are word-timed
  as the pane reads them (`wordTimesFor`). Text check against LRCLIB: same 48 lines and no
  added words. The only changes are your→you're, hoar→whore, "No Freedom"→"No freedom" and a
  stray trailing `"`.
- [x] "Suck it" removed 2026-11-10: Cameron says it isn't sung. It came from LRCLIB's record,
  not from the engine, which never heard it. So LRCLIB's text is wrong there too.
- [ ] Vocalizations after "Fuck it" (about 17–22 s): the engine heard nothing there. Waiting on
  Cameron to say what they are, then time them from the audio as a line like `(ooh, ooh)`.
- [x] Wings after Cameron's in-app edit (saved 1:23 PM PST): kept his "Throw down" and "theirs".
  Then finished "there's"→"theirs" in chorus 3 and put chorus 2's word timings back (lost by the
  editor bug in ISSUES.md). Restored the 1:02.30 pause, which had gone, and the section labels:
  the installed build predates `aecaa6f`, so its save rebuilt the plain text without them. **Until
  a release, every in-app save of a labelled song drops its labels.**
- [x] Obvious (EMELINE, an engine-written draft, override `fa876842023ee46e.json`): 23 lines had
  lost their word timings in edits. They were re-timed from the draft's heard words. Six barely
  heard hook lines copy the 0:46.63 hook's rhythm. 38 of 38 timed lines are word-timed now.
- [ ] Obvious: the two "Oh oh oh oh" lines Cameron added are untimed. They go either in the
  outro (after 2:06) or in the 0:49–0:54 and 1:50–1:55 gaps. Waiting on him to say where.
  Also asked whether "I can't aren't articulate it" is "I can't articulate it".
- [x] Recogniser: `app/src/sections.ts` (+ tests), Rust twin `autosync::text::is_section_label`.
- [x] Pane headings (`appendHead` in `main.ts`, `.lyric-section`): not `.line`, so the
  highlighter, flags and plain follow all keep counting rows only.
- [x] Autosync: a label has no words in `align_with` and starts a stanza. `align::VERSION` 2→3,
  so stored results are re-timed from the words they kept the next time each song is looked up.
- [x] Editor: label rows are styled as headings, the timing pass skips them, and save keeps
  them out of `synced` and the Lyricsfile and restores the old plain text's labels.
- [x] README.
- [x] **v1.21.0 released** 2026-11-10 at about 1:50 PM PST (21:50Z): section headings. Release
  run `42907531473` succeeded, with the installer, its signature and `latest.json`.
- [x] Obvious: "I can't articulate it" (Cameron confirmed). The "oh" lines are placed per the
  published lyrics (Dork: intro and outro, 3 then 4 oh's). The intro is one line at 0:00.88;
  its two groups couldn't be told apart in the audio. The outro is 2:10.63 and 2:18.26, cleared
  at 2:22.0. These lines are left unreviewed in the draft review state, for Cameron to check.
- Dropped: the Wings vocalizations after "Fuck it". Cameron doesn't remember them, and nothing
  will be invented.

## Findings, after building

- Flags left earlier on *plain* lyrics that contain labels address rows by index. Those rows
  have now shifted by the number of labels above them. Each flag keeps its own text, so nothing
  is lost, but such a flag may point one or two rows off. No flags on such songs were checked.
- With two languages in one plain file, headings are left out, because pairing changes which
  row is which.

## Things not to do

- Don't publish Wings to LRCLIB without Cameron's explicit yes.
- Don't put labels in LRC or Lyricsfile `lines`. Players show them as sung lines.
