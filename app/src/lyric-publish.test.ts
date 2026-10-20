// Run with `bun test`.
import { describe, expect, test } from "bun:test";
import { publicationFor } from "./lyric-publish";
import type { Lyrics } from "./lyric-editor";

const meta = { title: "Song (Radio Edit)", artist: "Band", album: "" };
const record: Lyrics = {
  synced: null,
  plain: "one\ntwo",
  source: "lrclib",
  id: 42,
  trackName: "Song",
  artistName: "The Band",
  albumName: "Album",
  duration: 201,
  overridden: false,
};
const body = { synced: "[00:01.00] one\n[00:03.00] two", plain: "one\ntwo" };

describe("publicationFor", () => {
  test("files under the matched record's own identity, not Pandora's naming", () => {
    const p = publicationFor(record, meta, 199, body)!;
    expect(p).toMatchObject({ trackName: "Song", artistName: "The Band", albumName: "Album", duration: 201 });
    expect(p).toMatchObject({ track: "Song (Radio Edit)", artist: "Band", album: null });
    expect(p.synced).toBe(body.synced);
  });

  test("new lyrics fall back to Pandora's names and the track length", () => {
    const fresh = { ...record, id: null, trackName: null, artistName: null, albumName: null, duration: null };
    expect(publicationFor(fresh, meta, 199, body)).toMatchObject({
      trackName: "Song (Radio Edit)",
      artistName: "Band",
      albumName: "Song (Radio Edit)",
      duration: 199,
    });
  });

  test("nothing to file without a length or without words", () => {
    expect(publicationFor({ ...record, duration: null }, meta, null, body)).toBeNull();
    expect(publicationFor(record, meta, 199, { synced: null, plain: null })).toBeNull();
  });
});
