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

## Later / proposals (need Cameron's yes)

- **Missed choruses in autosync:** when the engine hears nothing in a section the plain text says
  repeats, place it where the music repeats, using chorus 1's timings. Step 1 does this by hand;
  doing it natively would rescue songs like Wings automatically.

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
- [x] Recogniser: `app/src/sections.ts` (+ tests), Rust twin `autosync::text::is_section_label`.
- [x] Pane headings (`appendHead` in `main.ts`, `.lyric-section`): not `.line`, so the
  highlighter, flags and plain follow all keep counting rows only.
- [x] Autosync: a label has no words in `align_with` and starts a stanza. `align::VERSION` 2→3,
  so stored results are re-timed from the words they kept the next time each song is looked up.
- [x] Editor: label rows are styled as headings, the timing pass skips them, and save keeps
  them out of `synced` and the Lyricsfile and restores the old plain text's labels.
- [x] README.

## Findings, after building

- Flags left earlier on *plain* lyrics that contain labels address rows by index. Those rows
  have now shifted by the number of labels above them. Each flag keeps its own text, so nothing
  is lost, but such a flag may point one or two rows off. No flags on such songs were checked.
- With two languages in one plain file, headings are left out, because pairing changes which
  row is which.

## Things not to do

- Don't publish Wings to LRCLIB without Cameron's explicit yes.
- Don't put labels in LRC or Lyricsfile `lines`. Players show them as sung lines.
