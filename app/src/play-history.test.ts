// Run with `bun test`.
import { describe, expect, test } from "bun:test";
import { parse, playability, pushed, whyNot, type HistItem, type IndexedSong } from "./play-history";

const item = (title: string, extra: Partial<HistItem> = {}): HistItem => ({
  art: "",
  title,
  artist: "Artist",
  ...extra,
});
const saved = (id: string, title: string, stations: string[]): IndexedSong => ({
  id,
  title,
  artist: "Artist",
  stations,
});
const stations = [
  { name: "One", token: "ST:1" },
  { name: "Two", token: "ST:2" },
];

describe("pushed", () => {
  test("adds newest first and trims to the limit", () => {
    let h: HistItem[] = [];
    for (const t of ["a", "b", "c"]) h = pushed(h, item(t), 2);
    expect(h.map((x) => x.title)).toEqual(["c", "b"]);
  });
  test("a resumed track is not recorded twice", () => {
    const h = pushed([item("a", { id: "TR:a" })], item("a", { id: "TR:a" }));
    expect(h.length).toBe(1);
  });
  test("ids tell apart two recordings with the same name", () => {
    const h = pushed([item("a", { id: "TR:1" })], item("a", { id: "TR:2" }));
    expect(h.length).toBe(2);
  });
});

describe("parse", () => {
  test("survives junk", () => {
    expect(parse("nope")).toEqual([]);
    expect(parse('{"a":1}')).toEqual([]);
    expect(parse(null)).toEqual([]);
    expect(parse('[null, {"title":"a","artist":"b","art":""}, {"x":1}]').length).toBe(1);
  });
});

describe("playability", () => {
  const index = (songs: IndexedSong[]) => ({ enabled: true, songs });

  test("off when Saved songs is off", () => {
    expect(playability(item("a"), { enabled: false, songs: [] }, "ST:1", stations).kind).toBe("off");
  });

  test("unsaved when its audio is not on disk", () => {
    expect(playability(item("a", { id: "TR:a" }), index([]), "ST:1", stations).kind).toBe("unsaved");
  });

  test("prefers the station playing now, without switching", () => {
    const p = playability(
      item("a", { id: "TR:a", station: "ST:2" }),
      index([saved("TR:a", "a", ["ST:2", "ST:1"])]),
      "ST:1",
      stations
    );
    expect(p).toEqual({ kind: "play", id: "TR:a", station: { name: "One", token: "ST:1" }, switches: false });
  });

  test("otherwise the station it was played on, switching to it", () => {
    const p = playability(
      item("a", { id: "TR:a", station: "ST:2" }),
      index([saved("TR:a", "a", ["ST:9", "ST:2"])]),
      "ST:1",
      stations
    );
    expect(p).toEqual({ kind: "play", id: "TR:a", station: { name: "Two", token: "ST:2" }, switches: true });
  });

  test("gone when every station it was saved on has been deleted", () => {
    const p = playability(item("a", { id: "TR:a" }), index([saved("TR:a", "a", ["ST:9"])]), "ST:1", stations);
    expect(p.kind).toBe("gone");
  });

  test("any station is assumed to exist before the list arrives, using the recorded name", () => {
    const p = playability(
      item("a", { id: "TR:a", station: "ST:9", stationName: "Nine" }),
      index([saved("TR:a", "a", ["ST:9"])]),
      "",
      []
    );
    expect(p).toEqual({ kind: "play", id: "TR:a", station: { name: "Nine", token: "ST:9" }, switches: true });
  });

  test("old entries without an id match on folded title and artist", () => {
    const p = playability(
      { art: "", title: "Beyoncé", artist: "ARTIST" },
      index([saved("TR:b", "beyonce", ["ST:1"])]),
      "ST:1",
      stations
    );
    expect(p.kind).toBe("play");
  });
});

describe("whyNot", () => {
  test("silent when it can play, a sentence when it can't", () => {
    expect(whyNot({ kind: "play", id: "TR:a", station: { name: "One", token: "ST:1" }, switches: false })).toBe("");
    expect(whyNot({ kind: "unsaved" })).toStartWith("Not saved on this computer");
    expect(whyNot({ kind: "gone" })).toStartWith("Its station was deleted");
    expect(whyNot({ kind: "off" })).toStartWith("Saved songs is off");
  });
});
