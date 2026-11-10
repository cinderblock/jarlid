// Run with `bun test`.
import { describe, expect, test } from "bun:test";
import { sectionLabel, sectionsFor, splitSections, withSections } from "./sections";

describe("sectionLabel", () => {
  test("the spellings LRCLIB records use", () => {
    expect(sectionLabel("Chorus:")).toBe("Chorus");
    expect(sectionLabel("Verse 1:")).toBe("Verse 1");
    expect(sectionLabel("Pre-chorus:")).toBe("Pre-Chorus");
    expect(sectionLabel("[Chorus]")).toBe("Chorus");
    expect(sectionLabel("(Bridge)")).toBe("Bridge");
    expect(sectionLabel("  [Pre Chorus]  ")).toBe("Pre-Chorus");
    expect(sectionLabel("CHORUS")).toBe("Chorus");
    expect(sectionLabel("Outro")).toBe("Outro");
    expect(sectionLabel("Verse II")).toBe("Verse 2");
    expect(sectionLabel("[Post-Chorus]")).toBe("Post-Chorus");
  });

  test("who sings it, and how many times", () => {
    expect(sectionLabel("[Verse 2: Kendrick Lamar]")).toBe("Verse 2 · Kendrick Lamar");
    expect(sectionLabel("[Chorus - Both]")).toBe("Chorus · Both");
    expect(sectionLabel("Chorus x2")).toBe("Chorus ×2");
    expect(sectionLabel("[Chorus (x3)]")).toBe("Chorus ×3");
    expect(sectionLabel("2x Chorus")).toBe("Chorus ×2");
  });

  test("lyrics are lyrics", () => {
    expect(sectionLabel("The crowd threw me out for the kill")).toBeNull();
    expect(sectionLabel("Hook me up")).toBeNull();
    expect(sectionLabel("Chorus girls")).toBeNull();
    expect(sectionLabel("")).toBeNull();
    expect(sectionLabel("(oh oh oh)")).toBeNull();
    expect(sectionLabel("Verse after verse")).toBeNull();
    expect(sectionLabel("Remix2")).toBeNull();
    expect(sectionLabel("Chorusx2")).toBeNull();
    expect(sectionLabel("Chorus 2x")).toBe("Chorus ×2");
  });

  test("a name that could be sung needs brackets, a colon or a number", () => {
    expect(sectionLabel("Drop")).toBeNull();
    expect(sectionLabel("Solo")).toBeNull();
    expect(sectionLabel("[Drop]")).toBe("Drop");
    expect(sectionLabel("Solo:")).toBe("Solo");
    expect(sectionLabel("Hook 2")).toBe("Hook 2");
  });
});

describe("splitSections", () => {
  test("labels head the next line with words, and stanza gaps stay", () => {
    const { lines, heads } = splitSections(["Verse 1:", "a", "b", "", "Chorus:", "c", "Outro:"]);
    expect(lines).toEqual(["a", "b", "", "c"]);
    expect(heads).toEqual(["Verse 1", undefined, undefined, "Chorus"]);
  });
});

describe("sectionsFor", () => {
  const plain = ["Verse 1:", "one", "two", "Chorus:", "la la", "la di", "Verse 2:", "three", "Chorus:", "la la", "la di"].join("\n");

  test("timed lines get the labels of the plain lines they match, repeats in order", () => {
    const sung = ["one", "two", "La la!", "la di", "", "three", "la la", "la di"];
    expect(sectionsFor(plain, sung)).toEqual(["Verse 1", undefined, "Chorus", undefined, undefined, "Verse 2", "Chorus", undefined]);
  });

  test("an edited line still takes its label", () => {
    const sung = ["one", "two", "la la", "la di", "three (fixed)", "la la", "la di"];
    expect(sectionsFor(plain, sung)[4]).toBe("Verse 2");
  });

  test("a labelled line missing from the timed ones passes its label on", () => {
    const sung = ["one", "two", "la di", "three", "la la", "la di"];
    expect(sectionsFor(plain, sung)).toEqual(["Verse 1", undefined, "Chorus", "Verse 2", "Chorus", undefined]);
  });

  test("no labels, no headings", () => {
    expect(sectionsFor("one\ntwo", ["one", "two"])).toEqual([undefined, undefined]);
    expect(sectionsFor(null, ["one"])).toEqual([undefined]);
  });
});

describe("withSections", () => {
  test("puts the old plain text's labels back over edited lines", () => {
    const plain = "Verse 1:\none\ntwo\nChorus x2:\nla la";
    expect(withSections(plain, ["one", "two", "la la"])).toBe("[Verse 1]\none\ntwo\n\n[Chorus x2]\nla la");
  });

  test("labels the editor kept win", () => {
    expect(withSections("Chorus:\na", ["[Bridge]", "a"])).toBe("[Bridge]\na");
  });
});
