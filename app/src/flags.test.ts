// Run with `bun test`.
import { describe as group, expect, test } from "bun:test";
import {
  addMerged,
  appliesTo,
  describe,
  quickRows,
  rowAt,
  spanOf,
  type Flag,
  type Row,
  type Shown,
} from "./flags";

// Rows at 10, 14, a gap at 18, then 22 and 26.
const rows: Row[] = [
  { t: 10, text: "one" },
  { t: 14, text: "two" },
  { t: 18, text: "" },
  { t: 22, text: "three", alt: "trois" },
  { t: 26, text: "four" },
];

const shown = (over: Partial<Shown> = {}): Shown => ({
  source: "lrclib",
  id: 1,
  synced: "[00:10.00]one",
  plain: "one",
  auto: false,
  overridden: false,
  pinned: false,
  offset: 0,
  ...over,
});

let n = 0;
const flag = (first: number, last: number, over: Partial<Flag> = {}): Flag => ({
  id: `f${++n}`,
  created: n,
  span: spanOf(rows, first, last, 30),
  tags: [],
  note: "",
  shown: shown(),
  ...over,
});

group("rowAt", () => {
  test("follows the highlighter", () => {
    expect(rowAt(rows, 5)).toBe(-1);
    expect(rowAt(rows, 9.9)).toBe(0); // the highlighter's 0.15s lead
    expect(rowAt(rows, 15)).toBe(1);
    expect(rowAt(rows, 40)).toBe(4);
  });
});

group("quickRows", () => {
  test("the row playing, once it has been going a while", () => {
    expect(quickRows(rows, 16)).toEqual([1, 1]);
  });
  test("takes the row before when this one has only just started", () => {
    expect(quickRows(rows, 14.5)).toEqual([0, 1]);
  });
  test("the grace period skips back over a gap", () => {
    expect(quickRows(rows, 22.5)).toEqual([1, 3]);
  });
  test("in a gap, takes the last line sung with it", () => {
    expect(quickRows(rows, 20)).toEqual([1, 2]);
  });
  test("before the first line, the first line", () => {
    expect(quickRows(rows, 2)).toEqual([0, 0]);
  });
  test("nothing to flag in lyrics with no words", () => {
    expect(quickRows([{ t: 1, text: "" }], 5)).toBeNull();
  });
  test("leading blank rows are passed over", () => {
    const r: Row[] = [{ t: 0, text: "" }, { t: 5, text: "a" }, { t: 9, text: "b" }];
    expect(quickRows(r, 1)).toEqual([1, 1]);
  });
});

group("spanOf", () => {
  test("ends where the next timed row starts", () => {
    expect(spanOf(rows, 1, 0, 30)).toEqual({
      first: 0,
      last: 1,
      start: 10,
      end: 18,
      text: ["one", "two"],
    });
  });
  test("the last row runs to the song's end, and two languages read as one row", () => {
    const s = spanOf(rows, 3, 4, 30);
    expect(s.end).toBe(30);
    expect(s.text).toEqual(["three / trois", "four"]);
  });
  test("untimed rows have no times", () => {
    const plain: Row[] = [{ t: null, text: "a" }, { t: null, text: "b" }];
    expect(spanOf(plain, 0, 1, 30)).toMatchObject({ start: null, end: null });
  });
});

group("addMerged", () => {
  test("touching stretches on the same lyrics become one, keeping the oldest id", () => {
    const a = flag(0, 0, { tags: ["timing"], note: "early" });
    const b = flag(1, 1, { tags: ["words"], note: "typo" });
    const { flags, flag: merged } = addMerged([a], b, rows, 30);
    expect(flags).toHaveLength(1);
    expect(merged.id).toBe(a.id);
    expect(merged.span).toMatchObject({ first: 0, last: 1, start: 10, end: 18 });
    expect(merged.tags.sort()).toEqual(["timing", "words"]);
    expect(merged.note).toBe("early; typo");
  });
  test("a new stretch bridging two others swallows both", () => {
    const a = flag(0, 0);
    const c = flag(4, 4);
    const { flags, flag: merged } = addMerged([a, c], flag(1, 3), rows, 30);
    expect(flags).toHaveLength(1);
    expect(merged.span).toMatchObject({ first: 0, last: 4 });
  });
  test("only a verse break between them still counts as touching", () => {
    const { flags, flag: merged } = addMerged([flag(1, 1)], flag(3, 3), rows, 30);
    expect(flags).toHaveLength(1);
    expect(merged.span).toMatchObject({ first: 1, last: 3 });
  });
  test("separate stretches stay separate", () => {
    const { flags } = addMerged([flag(0, 0)], flag(3, 4), rows, 30);
    expect(flags).toHaveLength(2);
  });
  test("stretches on other lyrics are left alone", () => {
    const old = flag(0, 0, { shown: shown({ synced: "[00:11.00]one" }) });
    const { flags } = addMerged([old], flag(0, 1), rows, 30);
    expect(flags).toHaveLength(2);
  });
  test("re-adding a flag by id replaces it", () => {
    const a = flag(0, 0);
    const { flags } = addMerged([a], { ...a, span: spanOf(rows, 0, 3, 30) }, rows, 30);
    expect(flags).toHaveLength(1);
    expect(flags[0].span?.last).toBe(3);
  });
  test("whole-song flags never merge", () => {
    const whole = flag(0, 0, { span: null });
    const { flags } = addMerged([whole], flag(0, 0), rows, 30);
    expect(flags).toHaveLength(2);
  });
});

group("appliesTo", () => {
  const lrclib = { synced: "[00:10.00]one", plain: "one" };
  test("the same text", () => {
    expect(appliesTo(shown(), lrclib)).toBe(true);
    expect(appliesTo(shown(), { ...lrclib, synced: "[00:10.50]one" })).toBe(false);
  });
  test("auto-synced lyrics are the same rows whatever their times", () => {
    const s = shown({ auto: true, synced: "[00:10.00]one" });
    expect(appliesTo(s, { synced: "[00:12.00]one", plain: "one", auto: true })).toBe(true);
    expect(appliesTo(s, { synced: "[00:12.00]one!", plain: "one!", auto: true })).toBe(false);
    // Published since: real timings now, which the flag wasn't about.
    expect(appliesTo(s, { synced: "[00:12.00]one", plain: "one" })).toBe(false);
  });
});

group("describe", () => {
  test("reads for people", () => {
    expect(describe(flag(0, 0, { span: null }))).toBe("Whole song");
    expect(describe(flag(1, 1))).toBe("Line 2 · 0:14–0:18");
    expect(describe(flag(3, 4))).toBe("Lines 4–5 · 0:22–0:30");
  });
});
