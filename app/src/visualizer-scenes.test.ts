import { describe, expect, test } from "bun:test";
import { SCENE_BLEND_S, SCENE_MAX_S, SCENE_MIN_S, Scenes, pickScene, type Scene } from "./visualizer-scenes";

/** Scenes told apart by their zoom: the nth one picked has zoom n. */
function numbered() {
  let n = 0;
  return () => ({ ...pickScene(), zoom: n++ }) as Scene;
}

describe("scenes", () => {
  test("a quiet song changes scene at the longest age, without a beat", () => {
    const s = new Scenes(numbered());
    s.now(1000, 0);
    expect(s.now(1000 + SCENE_MAX_S + 1, 0).zoom).toBe(0);
    // Then blends into the next.
    expect(s.now(1000 + SCENE_MAX_S + 1 + SCENE_BLEND_S, 0).zoom).toBe(1);
  });

  test("the first frame long after the page loaded is the first scene, not a change", () => {
    const s = new Scenes(numbered());
    expect(s.now(5000, 0).zoom).toBe(0);
    expect(s.now(5001, 0).zoom).toBe(0);
  });

  test("a strong beat changes scene once it is old enough, not before", () => {
    const s = new Scenes(numbered());
    s.now(0, 0);
    s.now(SCENE_MIN_S - 1, 1);
    expect(s.now(SCENE_MIN_S - 1 + SCENE_BLEND_S, 0).zoom).toBe(0);
    s.now(SCENE_MIN_S + 1, 1);
    expect(s.now(SCENE_MIN_S + 1 + SCENE_BLEND_S, 0).zoom).toBe(1);
  });

  test("many changes in a row keep working", () => {
    const s = new Scenes(numbered());
    for (let t = 0; t < 600; t += 1 / 60) s.now(t, 0);
    expect(s.now(600 + SCENE_BLEND_S, 0).zoom).toBeGreaterThan(1);
  });
});
