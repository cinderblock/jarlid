// What a publish to LRCLIB carries. Shared by the timing editor and the lyrics pane's
// "Looks right? Publish" chip, so both file under the same record identity.

import type { Lyrics } from "./lyric-editor";
import type { TimedLine } from "./edit-lines";

/// Mirrors `Publication` in `lyrics.rs`.
export interface Publication {
  trackName: string;
  artistName: string;
  albumName: string;
  duration: number;
  synced: string | null;
  plain: string | null;
  /// Word timings and line ends; published as a Lyricsfile when present.
  timing?: TimedLine[] | null;
  /// How Pandora names the track, so the cache entry this supersedes can be dropped.
  artist: string;
  track: string;
  album: string | null;
}

/// LRCLIB identifies a record by track/artist/album/duration together, so all four are
/// required and must be the matched record's own wording — publishing under Pandora's
/// spelling files a new record beside the wrong one instead of correcting it. Pandora's
/// names stand in only when there is no record yet (new lyrics).
///
/// Null when there is nothing to file: no known length, or no words at all (which LRCLIB
/// would read as "this track is instrumental").
export function publicationFor(
  lyrics: Lyrics,
  meta: { title: string; artist: string; album: string },
  duration: number | null,
  body: { synced: string | null; plain: string | null; timing?: TimedLine[] | null }
): Publication | null {
  const length = lyrics.duration ?? duration;
  if (!length || (!body.synced && !body.plain)) return null;
  return {
    trackName: lyrics.trackName || meta.title,
    artistName: lyrics.artistName || meta.artist,
    albumName: lyrics.albumName || meta.album || meta.title,
    duration: length,
    ...body,
    artist: meta.artist,
    track: meta.title,
    album: meta.album || null,
  };
}
