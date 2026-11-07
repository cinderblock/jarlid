// Run with `bun test`.
import { describe, expect, test } from "bun:test";
import { clock, fold, matches, search, type SavedSong } from "./song-search";

const song = (title: string, artist: string, album = ""): SavedSong => ({
  id: `TR:${title}`,
  title,
  artist,
  album,
  length: 200,
  art: "",
  thumbUp: false,
  playedAt: 0,
});

describe("fold", () => {
  test("drops case, accents and punctuation", () => {
    expect(fold("Beyoncé")).toBe("beyonce");
    expect(fold("AC/DC")).toBe("ac dc");
    expect(fold("  Don't  Stop ")).toBe("don t stop");
  });
});

describe("matches", () => {
  const s = song("Back in Black", "AC/DC", "Back in Black");

  test("every word must appear somewhere", () => {
    expect(matches(s, "black acdc")).toBe(true);
    expect(matches(s, "ac dc")).toBe(true);
    expect(matches(s, "black sabbath")).toBe(false);
  });

  test("an empty query matches everything", () => {
    expect(matches(s, "")).toBe(true);
    expect(matches(s, "  ")).toBe(true);
  });

  test("accents need not be typed", () => {
    expect(matches(song("Halo", "Beyoncé"), "beyonce halo")).toBe(true);
  });
});

describe("search", () => {
  const songs = [
    song("Lovesong", "The Cure"),
    song("Song 2", "Blur"),
    song("Love Song", "Sara Bareilles"),
    song("Yellow", "Coldplay", "Parachutes"),
  ];

  test("titles that start with the query come first, then other title matches", () => {
    expect(search(songs, "song").map((s) => s.title)).toEqual(["Song 2", "Lovesong", "Love Song"]);
  });

  test("keeps the given order when nothing ranks higher", () => {
    expect(search(songs, "").map((s) => s.title)).toEqual(songs.map((s) => s.title));
  });

  test("finds by album", () => {
    expect(search(songs, "parachutes").map((s) => s.title)).toEqual(["Yellow"]);
  });
});

test("clock", () => {
  expect(clock(187)).toBe("3:07");
  expect(clock(3723)).toBe("1:02:03");
  expect(clock(0)).toBe("0:00");
});
