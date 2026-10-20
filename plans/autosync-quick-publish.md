# Publishing checked auto-synced timings from the lyrics pane

## Goal

Auto-synced timings (see `plans/auto-synced-lyrics.md`) never leave this machine unless
the listener publishes them. Publishing currently means opening the timing editor on
lyrics already checked by watching them, then pressing Publish there. This adds a
one-click "Looks right? Publish" chip to the lyrics pane while auto-synced lyrics are on
screen. It goes through the same confirm dialog and the same LRCLIB record identity as
the editor's Publish.

## Decisions already made (don't re-ask)

- **Never auto-publish.** Cameron asked whether auto-synced timings should be submitted
  automatically. The recommendation was no, and Cameron accepted it on 2026-10-20 ("i trust
  your rec"). The reasons:
  - LRCLIB has no delete, and every client gets the new revision.
  - Wrong alignments look plausible, so only a person watching can catch them.
  - Each publish costs about 2^24 hashes of proof-of-work during playback.
  - Silent publishing would leak listening activity, and it breaks the settings promise
    "nothing is published unless you choose to".
- **No "listened through without editing ⇒ publish" rule.** Listening isn't watching, so
  that rule is auto-publishing under another name.
- **A review queue for tracks that have already ended is out of scope.** It was offered as a
  bigger alternative, and the chip was the recommendation.

## Design

- **Where the chip appears.** `#lyrics-publish` is a quiet text chip in `.lyrics-head`,
  styled like the dual-language chip. It shows only when all of these hold:
  - the pane holds the *final* auto-synced result from `fetch_lyrics` (`auto: true`), not
    the progressive copy streamed while autosync is still working;
  - the lyrics belong to the track that is playing;
  - neither the editor nor the version picker is open;
  - the record has an LRCLIB `id` and a duration;
  - it was not already published this session.
- **What it publishes.** Exactly what the pane shows: the record's own plain lyrics plus the
  generated LRC. The identity is the matched record's (track, artist, album, duration),
  built by the same `publicationFor` helper the editor uses. Publishing under Pandora's
  spelling would file a new record instead of a revision.
- **Confirm first.** The same `#lyric-confirm` card asks first, with wording that says the
  timings were machine-made and should only be published if they were watched lining up.
- **After publishing**, the pane looks the track up again. LRCLIB normally serves the new
  synced lyrics at once (the editor's `settle()` has seen this). Either way the chip stays
  gone for that track for the rest of the session.
- **Confirm dialog.** It moved out of `lyric-editor.ts` into `lyric-confirm.ts` with a
  promise API, so the pane and the editor share one dialog. Escape is handled there.

## Progress log

- [x] Plan written.
- [x] `lyric-confirm.ts` extracted. The editor uses it with the same behaviour.
- [x] `publicationFor` shared between the editor and the chip.
- [x] Chip, visibility rules, publish flow and status in `main.ts`.
- [x] `bun run build`, `tsc --noEmit` and `bun test` pass. `lyric-publish.test.ts` covers the
      record identity.
- [x] README updated.
- [ ] Live check in the app: an auto-synced track shows the chip. Publishing goes through
      the dialog and LRCLIB then serves the timings. The chip is absent while syncing and
      during editing or picking.

## Findings / gotchas

- An auto-synced result always carries the LRCLIB record it was generated from. Autosync
  runs only where LRCLIB has plain lyrics, and `fetch_lyrics` keeps the record fields when
  it adds `auto: true`. So the chip always files a revision of an existing record and never
  a new one.
- `publish_lyrics` drops the disk cache entry, so the lookup after publishing goes to LRCLIB.
  The autosync stored result stays on disk, but it applies only while LRCLIB has no synced
  lyrics, so it stops showing on its own.
- The chip's state lives in `main.ts`'s top state block, not beside the chip code.
  `applyLyrics` reads it, and module-level `const`/`let` declared below a caller is a
  temporal dead zone (TDZ) trap.

## Things not to do

- Don't publish the progressive (still-syncing) copy. It isn't the gated result.
- Don't key the publication off Pandora's now-playing names when a record is matched.
