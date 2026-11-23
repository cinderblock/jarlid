# Issues

## Editing a line's words throws away its word timings

`lineTextChanged` in `app/src/edit-lines.ts:134` deletes a line's `words` whenever the edit
changes how many words it has. Fixing a speech-engine draft almost always does ("Only I can"
→ "Molly I can", "Hits your mark" → "Hit your mark" splits/merges), so a reviewed draft loses
word-by-word highlighting on most of the lines that needed fixing. Seen on Emei — "Molly"
(2026-11-09): 33 of 71 saved lines had no word timings, although the draft had times for
nearly every word.

What it should do instead: align the edited words with the old ones (or with the heard
words, `heard_words`) and keep or interpolate their times. A one-off script that did this
for Molly's saved edit worked well: a monotone alignment by spelling similarity, gaps spaced
evenly, and a line the engine didn't hear borrowing the rhythm of a line with the same words.

Partly eased (2026-11-15, plans/lyric-editor-text-editing.md): `editText` now keeps the times of
words an edit didn't touch and leaves only the touched ones untimed. A line with an untimed word
still saves without word timing (`toTiming` needs every word), so the alignment asked for here
is still open.

## `cargo clippy -D warnings` fails on code that predates the current toolchain

With clippy 1.100, `cargo clippy --all-targets -- -D warnings` fails in both workspaces on
lints the code was written before: `chunks_exact_to_as_chunks` (`crates/audio/src/player.rs`,
`prefetch.rs`), `neg_cmp_op_on_partial_ord` (`crates/audio/src/beats.rs`),
`manual_is_multiple_of`, unnecessary pointer casts (`app/src-tauri/src/lib.rs`), and dead
`Blend::rate_for` / `can_match` (`app/src-tauri/src/settings.rs`). Reproduce: run it in
`crates/` or `app/src-tauri/`. Matters because a real new warning is lost among them.
