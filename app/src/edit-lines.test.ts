import { describe, expect, test } from "bun:test";
import { applyReview, joinLine, reviewState, splitLine, unreviewed, type Line } from "./edit-lines";

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
