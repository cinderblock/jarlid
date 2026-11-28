// The visualizer's "scenes": how the picture zooms, turns and ripples, and how the wave is drawn
// into it. Every so often, on a beat, the picture drifts into a new one. Pure, so it can be
// tested without a GPU (visualizer-render.ts draws them).

/** How the feedback moves, and how the wave is drawn into it. Rates are per 60 Hz frame. */
export interface Scene {
  /** Above 1 the picture flows outward from the centre; below, it falls inward. */
  zoom: number;
  /** Turn per frame, in radians. */
  rot: number;
  /** How far, and how finely, the picture ripples. */
  warp: number;
  warpFreq: number;
  /** How much of the last frame survives into this one. */
  decay: number;
  /** Colour drift per frame, in radians of hue. */
  hueShift: number;
  /** 0 a line across, 1 a ring, 2 a spiral. */
  waveMode: number;
  waveSize: number;
  cx: number;
  cy: number;
  /** A mirrored, zoomed copy laid over the picture; 0 for none. */
  echo: number;
}

export function pickScene(r: () => number = Math.random): Scene {
  return {
    zoom: r() < 0.2 ? 0.985 + r() * 0.01 : 1.004 + r() * 0.022,
    rot: (r() - 0.5) * 0.014,
    warp: r() * 0.006,
    warpFreq: 2 + r() * 10,
    decay: 0.94 + r() * 0.045,
    hueShift: (r() - 0.5) * 0.012,
    waveMode: Math.floor(r() * 3),
    waveSize: 0.7 + r() * 0.6,
    cx: 0.5 + (r() - 0.5) * 0.12,
    cy: 0.5 + (r() - 0.5) * 0.12,
    echo: r() < 0.35 ? 0.3 + r() * 0.4 : 0,
  };
}

/** A scene lasts at least this long, and changes on the next strong beat after it… */
export const SCENE_MIN_S = 14;
/** …or at this age regardless. */
export const SCENE_MAX_S = 30;
export const SCENE_BLEND_S = 3;
/** The onset pulse that counts as a strong beat. */
const STRONG_BEAT = 0.7;

export class Scenes {
  private from: Scene;
  private to: Scene;
  /** When the current blend began; null until the first frame, so the first scene's age
   *  counts from when the view starts drawing, not from when the page loaded. */
  private at: number | null = null;

  constructor(private readonly pick: () => Scene = pickScene) {
    this.from = pick();
    this.to = this.from;
  }

  /** The scene at time `t` (seconds): easing from the last one to the next, the wave's shape
   *  swapping halfway. A new one starts on a strong beat once this one is old enough. */
  now(t: number, pulse: number): Scene {
    this.at ??= t;
    const age = t - this.at;
    if ((age > SCENE_MIN_S && pulse > STRONG_BEAT) || age > SCENE_MAX_S) {
      // The next blend starts from wherever this one has got to.
      this.from = this.blend(age);
      this.to = this.pick();
      this.at = t;
      return this.from;
    }
    return this.blend(age);
  }

  private blend(age: number): Scene {
    const k = Math.min(1, Math.max(0, age / SCENE_BLEND_S));
    const e = k * k * (3 - 2 * k);
    const a = this.from;
    const b = this.to;
    const mix = (x: number, y: number) => x + (y - x) * e;
    return {
      zoom: mix(a.zoom, b.zoom),
      rot: mix(a.rot, b.rot),
      warp: mix(a.warp, b.warp),
      warpFreq: mix(a.warpFreq, b.warpFreq),
      decay: mix(a.decay, b.decay),
      hueShift: mix(a.hueShift, b.hueShift),
      waveMode: e < 0.5 ? a.waveMode : b.waveMode,
      waveSize: mix(a.waveSize, b.waveSize),
      cx: mix(a.cx, b.cx),
      cy: mix(a.cy, b.cy),
      echo: mix(a.echo, b.echo),
    };
  }
}
