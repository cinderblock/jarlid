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

## A line's start can move without its first word

Word 0 is meant to be the line's start (`setStart` in `app/src/edit-lines.ts` moves both),
but some other path moves only the line. Molly's saved edit (2026-11-09 2:11 PM PST) had
"Cause it's gonna be alright" starting at 2:41.644 while its first word stayed at 2:41.75.
Not reproduced; noticed in the saved JSON.
