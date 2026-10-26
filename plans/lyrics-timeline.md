# Lyrics timeline: see the song, drag the timings

## Goal

Cameron, 2026-10-26: "can we have a timeline view when doing lyrics/time matching? might
make it easier to visually match the waveform of the music to the words. we could even more
clearly time each individual word - or capture the end of verses and the pause to the next".

A timeline in the lyric editor: the song's waveform, the words the speech engine heard, and
the lyric lines as blocks whose edges are dragged into place. Lines get **ends** (verse ends,
pauses before the next line), and optionally **word-by-word** timing.

Related: `plans/draft-lyrics.md` (draft review, which this makes faster),
`plans/lyrics-corrections.md` (the inline editor it extends).

## Environment / context

- Editor: `app/src/lyric-editor.ts` (DOM, modes) + `app/src/edit-lines.ts` (pure line ops,
  tested). Lines are `{t, text, doubt?, ok?}`; LRC is serialized there.
- Pane LRC parser: `app/src/lrc.ts`; Rust LRC: `crates/autosync/src/lrc.rs`.
- Audio for a waveform: `audio::Decoder::open_at(src, Some(16 kHz mono))` already decodes a
  whole track fast (`decode_to_wav` in `app/src-tauri/src/autosync/mod.rs`); source is the
  cached file (`engine.cached_file`) or the stream URL.
- Heard words: stored for auto-synced results (`Stored.heard`, no `p`) and drafts
  (`StoredDraft.heard`, with `p`) under the cache dir.
- **A pause already has a form:** a timed blank line ends the line before it. The pane, the
  editor and LRCLIB all handle it today, so line ends need no new format.
- **LRCLIB has native word timing: Lyricsfile** (YAML, spec at
  <https://github.com/tranxuanthang/lyricsfile/blob/main/SPECIFICATION.md>, introduced with
  LRCGET 2.0). Read from LRCLIB's source (`server/src/routes/publish_lyrics.rs`,
  `server/src/lyricsfile.rs`, fetched 2026-10-26):
  - `POST /api/publish` takes `lyricsfile: Option<String>` beside `plainLyrics` /
    `syncedLyrics`. **If `lyricsfile` is non-blank, the legacy fields sent are ignored**: the
    server derives `syncedLyrics` and `plainLyrics` from the Lyricsfile and stores all three.
  - Derived `syncedLyrics`: one `[mm:ss.xx]text` per Lyricsfile line (no space after the
    stamp), text = words joined if the line has words, else `text`. **`end_ms` is not used**,
    so a pause reaches old players only as an explicit line with empty text.
  - Derived `plainLyrics`: the document's `plain` if non-blank, else the lines' texts joined.
  - `has_word_sync` = any line with a non-empty `words` list.
  - **A Lyricsfile that fails to parse is stored as-is with no plain or synced lyrics** —
    the record would go blank for every old client. Validate before publishing.
  - `/api/get`, `/api/get/{id}` and `/api/search` return `lyricsfile` (and, per
    third-party reports, `hasWordSync`).
  - Format: `version: '1.0'`, `metadata {title, artist, album?, duration_ms?, instrumental}`,
    `lines [{text, start_ms, end_ms?, words? [{text (with trailing space), start_ms,
    end_ms?}]}]`, `plain?` (literal block). Integer milliseconds.
- Sources: LRCGET #421, Luminous #1237, dynamic-music-pill #193 (word syncs reach clients
  only via `lyricsfile`), LRCLIB #129.

## Design (draft)

1. **Backend `track_envelope`**: decode the edited track at 16 kHz mono on a blocking thread,
   return a loudness envelope (RMS per 20 ms, ~50 values/s, a few KB for a song), cached per
   track for the session.
2. **Backend `heard_words`**: what the engine heard for this track, when it has been heard.
3. **Timeline panel** in edit mode (toggle in the lyric bar), canvas-drawn, horizontally
   scrollable, follows the playhead, zoom with Ctrl+wheel / pinch:
   - lane 1: waveform; click to seek.
   - lane 2: heard words (faint chips; doubtful ones underlined).
   - lane 3: lyric lines as blocks, start → end (end = the next line's start, or a pause).
     Drag left edge = start; drag right edge = end (writes/updates the blank line after it);
     drag the body = shift the line. Selecting a block selects the row in the list and back.
   - lane 4 (selected line): its words as sub-blocks with draggable boundaries.
4. **Data**: `Line` gains `words?: (number|null)[]` (start of each word, by position). Ends stay
   blank timed lines. Editing a line's text drops word times if the word count changes.
5. **Word timing source**: from the drag handles; seeded from heard words where the line's
   words match them (drafts: exactly; aligned: by the aligner's matches).

## Decisions already made (don't re-ask)

- **Publish word timings too, without breaking older players** (Cameron, 2026-10-26).
  → Publish a Lyricsfile; LRCLIB derives line-level `syncedLyrics` for old clients. Every
  pause is written twice: `end_ms` on the line *and* an empty timed line after it, so old
  clients still see the gap.

## Plan / steps

1. [x] Word-timing question: publish via Lyricsfile (above).
2. [ ] Envelope + heard-words commands, tests.
3. [ ] Timeline panel: waveform, playhead, seek, zoom, line blocks with start/end dragging.
4. [ ] Lyricsfile in Rust (`serde_yaml`): parse what LRCLIB serves into `Lyrics.timing`
   (lines with ends and words, as JSON to the frontend); build one from the editor's lines
   for saving and publishing; validate by re-parsing it the way LRCLIB does before sending.
5. [ ] Word lane in the timeline; drafts seeded with the engine's word times.
6. [ ] Pane: word-by-word highlight where words are timed.
7. [ ] README, plan.

## Open questions for the user

(none)

## Findings / gotchas

(none yet)

## Things not to do

- Don't invent a new format for line ends; a timed blank line already is one.
- No native controls, no `title=` (project rule).
