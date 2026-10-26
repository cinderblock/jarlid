import { describe, expect, test } from "bun:test";
import {
  applyReview,
  endsInPause,
  fromTiming,
  lineEnd,
  mergeTiming,
  joinLine,
  retext,
  reviewState,
  setEnd,
  setStart,
  setWordStart,
  splitLine,
  toTiming,
  unreviewed,
  wordStarts,
  wordTimesFor,
  type Line,
} from "./edit-lines";

const draft = (): Line[] => {
  const lines: Line[] = [
    { t: 10, text: "I took a walk around the world" },
    { t: 14, text: "to ease my troubled mind" },
    { t: 18, text: "" },
    { t: 30, text: "I left my body lying" },
  ];
  applyReview(lines, {
    doubt: [[0], [], [], [2, 3]],
    reviewed: [true, false, false, false],
  });
  return lines;
};

describe("reviewing a draft", () => {
  test("blank lines count as reviewed, and publishing waits for the rest", () => {
    const lines = draft();
    expect(lines.map((l) => l.ok)).toEqual([true, false, true, false]);
    expect(unreviewed(lines)).toBe(2);
    expect(reviewState(lines)?.reviewed).toEqual([true, false, true, false]);

    lines[1].ok = lines[3].ok = true;
    // A finished review leaves nothing to record, which is what lets it be published.
    expect(reviewState(lines)).toBeNull();
  });

  test("splitting a line makes both halves new, timed between it and the next", () => {
    const lines = draft();
    const text = lines[0].text;
    splitLine(lines, 0, text, text.indexOf("around"), true);
    expect(lines.map((l) => l.text).slice(0, 2)).toEqual(["I took a walk", "around the world"]);
    expect(lines[0].ok).toBe(false);
    expect(lines[1].ok).toBe(false);
    expect(lines[0].doubt).toEqual([]);
    // 13 of 29 characters in, between 10 s and the next line's 14 s.
    expect(lines[1].t).toBeCloseTo(10 + (4 * 13) / 29, 6);
  });

  test("Enter at the end opens an empty line and leaves the line's review alone", () => {
    const lines = draft();
    splitLine(lines, 0, lines[0].text, lines[0].text.length, true);
    expect(lines[1]).toEqual({ t: null, text: "", ok: false });
    expect(lines[0].ok).toBe(true);
    expect(lines[0].doubt).toEqual([0]);
  });

  test("joining onto the line above un-reviews it and says where the join is", () => {
    const lines = draft();
    const at = joinLine(lines, 1, lines[1].text, true);
    expect(lines[0].text).toBe("I took a walk around the world to ease my troubled mind");
    expect(at).toBe("I took a walk around the world".length + 1);
    expect(lines[0].ok).toBe(false);
    expect(lines[0].t).toBe(10);
    expect(lines).toHaveLength(3);
  });

  test("joining onto a blank line just drops the blank", () => {
    const lines = draft();
    expect(joinLine(lines, 3, lines[3].text, true)).toBe(0);
    expect(lines.map((l) => l.text)).toEqual([
      "I took a walk around the world",
      "to ease my troubled mind",
      "I left my body lying",
    ]);
    expect(lines[2].t).toBe(30);
    expect(lines[2].doubt).toEqual([2, 3]);
  });
});

describe("outside a draft", () => {
  test("splitting an untimed line gives an untimed one, with no review state", () => {
    const lines: Line[] = [{ t: null, text: "one two three four" }];
    splitLine(lines, 0, lines[0].text, 8, false);
    expect(lines).toEqual([
      { t: null, text: "one two" },
      { t: null, text: "three four" },
    ]);
  });

  test("the last timed line splits into the four seconds after it", () => {
    const lines: Line[] = [{ t: 100, text: "abcd efgh" }];
    splitLine(lines, 0, lines[0].text, 5, false);
    expect(lines[1].t).toBeCloseTo(100 + (4 * 4) / 8, 6);
  });
});

describe("word timings", () => {
  const timed = (): Line[] => [
    { t: 10, text: "I took a walk", words: [10, 10.4, 10.7, 11] },
    { t: 13, text: "around the world", words: [13, 13.5, 13.9] },
  ];

  test("a split keeps each word's time, and the second half starts at its first word", () => {
    const lines = timed();
    splitLine(lines, 0, lines[0].text, "I took".length, false);
    expect(lines[0]).toEqual({ t: 10, text: "I took", words: [10, 10.4] });
    expect(lines[1]).toEqual({ t: 10.7, text: "a walk", words: [10.7, 11] });
  });

  test("a join keeps both lines' word times", () => {
    const lines = timed();
    joinLine(lines, 1, lines[1].text, false);
    expect(lines[0].words).toEqual([10, 10.4, 10.7, 11, 13, 13.5, 13.9]);
  });

  test("a spelling fix keeps them; a different number of words drops them", () => {
    const lines = timed();
    retext(lines[0], "I took a wlak");
    expect(lines[0].words).toHaveLength(4);
    retext(lines[0], "I took a long walk");
    expect(lines[0].words).toBeUndefined();
  });

  test("to and from a Lyricsfile's timing, with a pause between the lines", () => {
    const lines: Line[] = [...timed()];
    lines.splice(1, 0, { t: 12, text: "" });
    const timing = toTiming(lines)!;
    expect(timing[0]).toEqual({
      text: "I took a walk",
      start: 10,
      end: 12,
      words: [
        { text: "I", start: 10 },
        { text: "took", start: 10.4 },
        { text: "a", start: 10.7 },
        { text: "walk", start: 11 },
      ],
    });
    expect(timing[1]).toEqual({ text: "", start: 12 });
    expect(fromTiming(timing)).toEqual(lines);
  });

  test("an end with no empty line after it becomes one", () => {
    const back = fromTiming([
      { text: "One", start: 1, end: 2 },
      { text: "Two", start: 5 },
    ]);
    expect(back.map((l) => [l.t, l.text])).toEqual([
      [1, "One"],
      [2, ""],
      [5, "Two"],
    ]);
  });

  test("no word times means the LRC says it all", () => {
    expect(toTiming([{ t: 1, text: "One" }, { t: 2, text: "" }])).toBeNull();
    // A line with an untimed word is published without words; the others keep theirs.
    const t = toTiming([
      { t: 1, text: "One two", words: [1, null] },
      { t: 3, text: "Three", words: [3] },
    ])!;
    expect(t[0].words).toBeUndefined();
    expect(t[1].words).toEqual([{ text: "Three", start: 3 }]);
  });
});

describe("the timeline's edits", () => {
  const song = (): Line[] => [
    { t: 10, text: "I took a walk" },
    { t: 14, text: "around the world" },
    { t: 20, text: "" },
    { t: 30, text: "to ease my mind" },
  ];

  test("a start moves only between its neighbours", () => {
    const lines = song();
    expect(setStart(lines, 1, 12)).toBe(12);
    expect(setStart(lines, 1, 5)).toBeCloseTo(10.05, 6);
    expect(setStart(lines, 1, 25)).toBeCloseTo(19.95, 6);
  });

  test("dragging an end before the next line makes a pause; up to it removes one", () => {
    const lines = song();
    expect(endsInPause(lines, 0)).toBe(false);
    expect(setEnd(lines, 0, 12.5)).toBe(1);
    expect(lines[1]).toEqual({ t: 12.5, text: "" });
    expect(lineEnd(lines, 0, 999)).toBe(12.5);
    // Moving it again moves the same pause, not a second one.
    expect(setEnd(lines, 0, 13)).toBe(1);
    expect(lines).toHaveLength(5);
    // Up against the next line: no pause any more.
    expect(setEnd(lines, 0, 14)).toBe(-1);
    expect(lines.map((l) => l.text)).toEqual(song().map((l) => l.text));
  });

  test("an existing pause moves, bounded by the line after it", () => {
    const lines = song();
    expect(setEnd(lines, 1, 18)).toBe(2);
    expect(lines[2].t).toBe(18);
    expect(setEnd(lines, 1, 40)).toBe(-1);
    expect(lines.some((l, i) => i > 0 && l.text === "" && l.t === 20)).toBe(false);
  });

  test("untimed words share the line evenly; dragging one keeps what was shown", () => {
    const lines = song();
    expect(wordStarts(lines, 0, 999)).toEqual([10, 11, 12, 13]);
    setWordStart(lines, 0, 2, 12.5, 999);
    expect(lines[0].words).toEqual([10, 11, 12.5, 13]);
    // Not past the next word.
    expect(setWordStart(lines, 0, 2, 20, 999)).toBeCloseTo(12.95, 6);
    // Word 0 is the line's start.
    setWordStart(lines, 0, 0, 9, 999);
    expect(lines[0].t).toBe(9);
    expect(lines[0].words![0]).toBe(9);
  });

  test("words between timed ones are spread between them", () => {
    const lines: Line[] = [{ t: 0, text: "a b c d e", words: [0, null, null, 6, null] }, { t: 10, text: "x" }];
    expect(wordStarts(lines, 0, 99)).toEqual([0, 2, 4, 6, 8]);
  });
});

describe("merging a Lyricsfile's timing onto the LRC's lines", () => {
  test("words attach by start and text; untimed lines keep their places", () => {
    const lrc: Line[] = [
      { t: 10, text: "I took a walk" },
      { t: null, text: "an untimed line" },
      { t: 14, text: "around the world" },
    ];
    const merged = mergeTiming(lrc, [
      { text: "I took a walk", start: 10.004, end: 12, words: [
        { text: "I", start: 10 }, { text: "took", start: 10.4 }, { text: "a", start: 10.7 }, { text: "walk", start: 11 },
      ] },
      { text: "around the world", start: 14 },
    ]);
    expect(merged.map((l) => [l.t, l.text])).toEqual([
      [10, "I took a walk"],
      [12, ""],
      [null, "an untimed line"],
      [14, "around the world"],
    ]);
    expect(merged[0].words).toEqual([10, 10.4, 10.7, 11]);
    // The input is left alone.
    expect(lrc[0].words).toBeUndefined();
  });

  test("an end that is already an empty line isn't doubled", () => {
    const merged = mergeTiming(
      [{ t: 1, text: "One" }, { t: 2, text: "" }, { t: 5, text: "Two" }],
      [{ text: "One", start: 1, end: 2 }, { text: "", start: 2 }, { text: "Two", start: 5 }]
    );
    expect(merged).toHaveLength(3);
  });
});

test("the pane's lines find their word times by start and text", () => {
  const times = wordTimesFor(
    [{ t: 1, text: "One two" }, { t: 3, text: "" }, { t: 5, text: "Three" }],
    [
      { text: "One two", start: 1.004, words: [{ text: "One", start: 1 }, { text: "two", start: 1.5 }] },
      { text: "Three", start: 5 },
    ]
  );
  expect(times).toEqual([[1, 1.5], undefined, undefined]);
});
