// Run with `bun test`. The lines are made up for the tests; each shape is copied from a real
// LRCLIB record (named beside it), not the words.
import { describe, expect, test } from "bun:test";
import { detectDual, guessLanguage, type DualLine } from "./dual-lyrics";

const timed = (texts: string[]): DualLine[] => texts.map((text, i) => ({ t: i * 4, text }));

const FR = [
  "Je marche dans la rue le soir",
  "Tu es tout ce que je veux",
  "Le ciel est devenu gris pour moi",
  "Je ne sais pas où tu vas",
  "On danse jusqu'au matin",
  "Mais rien dont j'ai besoin",
];
const EN = [
  "I walk along the street at night",
  "You're all that I could want",
  "The sky has turned to grey for me",
  "I don't know where you're going",
  "We dance until the morning light",
  "But nothing that I need",
];

describe("detectDual", () => {
  // Madelline - Dopamine (Split Brain Version), LRCLIB 35090140: `FR - EN`.
  test("FR - EN", () => {
    const d = detectDual(timed([...FR.map((f, i) => `${f} - ${EN[i]}`), "Ooh-ooh-ooh", "Da-ra-da-da-da"]));
    expect(d).not.toBeNull();
    expect(d!.labels).toEqual(["FR", "EN"]);
    expect(d!.lines[0]).toMatchObject({ text: FR[0], alt: EN[0], t: 0 });
    // Filler stays one line.
    expect(d!.lines[6].alt).toBeUndefined();
  });

  // LRCLIB 11043109: `EN (FR)`.
  test("EN (FR)", () => {
    const d = detectDual(timed(EN.map((e, i) => `${e} (${FR[i]})`)));
    expect(d?.labels).toEqual(["EN", "FR"]);
    expect(d!.lines[1]).toMatchObject({ text: EN[1], alt: FR[1] });
  });

  // A line that reads the same in both languages counts neither way, and is still split.
  test("lines alike in both languages don't sink a file", () => {
    const alike = Array(6).fill("Da-ra-da-da, ma D-O-P-A-M-I-N-E (da-ra-da-da, my D-O-P-A-M-I-N-E)");
    const d = detectDual(timed([...FR.map((f, i) => `${f} (${EN[i]})`), ...alike]));
    expect(d).not.toBeNull();
    expect(d!.lines[8].alt).toBe("da-ra-da-da, my D-O-P-A-M-I-N-E");
  });

  test("two lines on one timestamp", () => {
    const lines = FR.flatMap((f, i) => [
      { t: i * 4, text: f },
      { t: i * 4, text: EN[i] },
    ]);
    const d = detectDual(lines);
    expect(d?.lines).toHaveLength(6);
    expect(d!.lines[2]).toMatchObject({ t: 8, text: FR[2], alt: EN[2] });
  });

  test("non-Latin script with a translation", () => {
    const ko = ["나는 밤에 거리를 걸어", "너는 내가 원하는 전부야", "하늘이 회색으로 변했어", "네가 어디 가는지 몰라", "아침까지 춤을 춰", "하지만 필요한 건 없어"];
    const d = detectDual(timed(ko.map((k, i) => `${k} / ${EN[i]}`)));
    expect(d?.labels).toEqual(["KO", "EN"]);
  });

  // The traps. Ordinary songs must come back untouched.
  test("echoes in brackets are backing vocals", () => {
    expect(
      detectDual(
        timed([
          "I keep on running back to you (back to you)",
          "Every single night I call your name (call your name)",
          "Don't you leave me now (leave me now)",
          "Hold me close tonight (hold me close)",
          "We were young and free (young and free)",
          "Nothing stays the same (stays the same)",
        ])
      )
    ).toBeNull();
  });

  test("vocables in brackets are backing vocals", () => {
    expect(
      detectDual(
        timed([
          "I keep on running back to you (ooh yeah)",
          "Every single night I call your name (oh-oh)",
          "Don't you leave me now (na-na-na)",
          "Hold me close tonight (hey)",
          "We were young and free (ah-ah)",
          "Nothing stays the same (mmm)",
        ])
      )
    ).toBeNull();
  });

  test("an English answer line in brackets is not a translation", () => {
    expect(
      detectDual(
        timed([
          "I told you that I'd wait for you (and you know that it's true)",
          "When the lights go down in the city (we will be there with you)",
          "Is it me that you were looking for (is it you that I adore)",
          "Take the long way home tonight (you and I will be alright)",
          "All the things we said in the dark (all the things that we are)",
          "If you want it come and get it (we can make it if we try)",
        ])
      )
    ).toBeNull();
  });

  test("a few split lines don't make a dual file", () => {
    const lines = timed([`${FR[0]} - ${EN[0]}`, ...EN, ...EN]);
    expect(detectDual(lines)).toBeNull();
  });

  test("three voices on one timestamp are a chorus, not a translation", () => {
    const lines = FR.flatMap((f, i) => [
      { t: i * 4, text: f },
      { t: i * 4, text: EN[i] },
      { t: i * 4, text: "Hey hey hey" },
    ]);
    expect(detectDual(lines)).toBeNull();
  });
});

describe("guessLanguage", () => {
  test("tells the common ones apart", () => {
    expect(guessLanguage(FR.join("\n"))).toBe("fr");
    expect(guessLanguage(EN.join("\n"))).toBe("en");
    expect(guessLanguage("愛は泣いていたんだろう秘密の場所で")).toBe("ja");
  });
  test("says nothing rather than guess from too little", () => {
    expect(guessLanguage("Hello")).toBeNull();
  });
});
