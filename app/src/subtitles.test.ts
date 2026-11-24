import { describe, expect, test } from "bun:test";
import { lineText, wordAt, wordTimes, type SubLine } from "./subtitles";

const lines: SubLine[] = [
  { t: 10, text: "hello there world", words: null },
  { t: 12, text: "", words: null },
  { t: 20, text: "timed words", words: [20, 20.8] },
  { t: 30, text: "a long line into a gap", words: null },
];

describe("line mode", () => {
  test("nothing before the first line", () => expect(lineText(lines, 5)).toBe(""));
  test("the line being sung", () => expect(lineText(lines, 11)).toBe("hello there world"));
  test("a pause shows nothing", () => expect(lineText(lines, 15)).toBe(""));
});

describe("word mode", () => {
  test("words without times are spread towards the next line", () => {
    const close: SubLine[] = [
      { t: 10, text: "one two three", words: null },
      { t: 11.5, text: "next", words: null },
    ];
    expect(wordTimes(close, 0)).toEqual([10, 10.5, 11]);
  });
  test("word times are used when the line has them", () => {
    expect(wordAt(lines, 20.5)?.word).toBe("timed");
    expect(wordAt(lines, 20.9)?.word).toBe("words");
  });
  test("a line running into a gap gives each word at most 0.6 s", () => {
    const times = wordTimes(lines, 3);
    expect(times[5]).toBeCloseTo(33);
  });
  test("the last word goes once it has been held", () => {
    expect(wordAt(lines, 34)?.word).toBe("gap");
    expect(wordAt(lines, 40)).toBeNull();
  });
  test("the same word twice is two words", () => {
    const twice: SubLine[] = [{ t: 0, text: "go go", words: [0, 1] }];
    expect(wordAt(twice, 0.5)?.key).not.toBe(wordAt(twice, 1.5)?.key);
  });
});
