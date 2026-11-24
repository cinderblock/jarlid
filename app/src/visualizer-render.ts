/// <reference types="@webgpu/types" />
// The visualizer's renderer, for any canvas: the lyrics pane, behind the main window, or the
// visualizer's own window (visualizer.ts and visualizer-window.ts place them).
//
// The page has no audio of its own (Rust decodes and plays it), so the spectrum and waveform
// are measured in Rust (visualizer.rs) from what the speaker is playing, and arrive sixty times
// a second as raw floats over a Tauri channel. One channel per page, open while any view on the
// page is showing.
//
// Drawn with WebGPU as a feedback loop, the way MilkDrop and the old iTunes visualizer drew:
// each frame starts from the last one, zoomed, turned and rippled a little, its colours shifted
// and fading, and the waveform is drawn into it fresh. What the song did a moment ago streams
// away from the centre in trails. How it zooms, turns and ripples, and how the wave is drawn,
// is a "scene"; every so often, on a beat, the picture drifts into a new one.
//
// If WebGPU won't start, the view says so rather than quietly showing nothing.

import { Channel, invoke } from "@tauri-apps/api/core";

const BANDS = 64;
const WAVE = 256;
/** Per frame from Rust (visualizer.rs): the bands, the level, the low end, the onset pulse,
 *  then the waveform. */
const FRAME_LEN = BANDS + 3 + WAVE;
/** Matches the views' fade in styles.css, so drawing stops only once it is out of sight. */
const FADE_MS = 600;

// ---- the analysis feed, one per page -----------------------------------------------

let latest = new Float32Array(FRAME_LEN);
let feeding = 0;

// A reloaded page leaves the last one's analysis running with nobody to draw it.
void invoke("visualizer_stop");

async function subscribe() {
  if (feeding++ > 0) return;
  const frames = new Channel<ArrayBuffer>();
  frames.onmessage = (data) => {
    if (feeding > 0) latest = new Float32Array(data);
  };
  await invoke("visualizer_start", { frames });
}

function unsubscribe() {
  if (--feeding > 0) return;
  void invoke("visualizer_stop");
  // Whatever is still fading out does so with the music falling away.
  latest = new Float32Array(FRAME_LEN);
}

// ---- scenes ------------------------------------------------------------------

/** How the feedback moves, and how the wave is drawn into it. Rates are per 60 Hz frame. */
interface Scene {
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

function pickScene(): Scene {
  const r = Math.random;
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
const SCENE_MIN_S = 14;
/** …or at this age regardless. */
const SCENE_MAX_S = 30;
const SCENE_BLEND_S = 3;

class Scenes {
  private from = pickScene();
  private to = this.from;
  private at = 0;

  /** The scene now: easing from the last one to the next, the wave's shape swapping halfway. */
  now(t: number, pulse: number): Scene {
    const age = t - this.at;
    if ((age > SCENE_MIN_S && pulse > 0.7) || age > SCENE_MAX_S) {
      this.from = this.now(t, 0);
      this.to = pickScene();
      this.at = t;
      return this.from;
    }
    const k = Math.min(1, age / SCENE_BLEND_S);
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

// ---- WebGPU, shared by the page's views ------------------------------------------

/** The feedback image: two textures, each frame drawn into one from the other. */
const FEEDBACK_FORMAT: GPUTextureFormat = "rgba16float";

interface Core {
  device: GPUDevice;
  format: GPUTextureFormat;
  warp: GPURenderPipeline;
  wave: GPURenderPipeline;
  present: GPURenderPipeline;
  sampler: GPUSampler;
}

let core: Promise<Core> | null = null;
const views = new Set<VisualizerView>();

async function initCore(): Promise<Core> {
  if (!navigator.gpu) throw new Error("navigator.gpu is missing");
  const adapter = await navigator.gpu.requestAdapter();
  if (!adapter) throw new Error("no GPU adapter");
  const device = await adapter.requestDevice();
  device.lost.then((info) => {
    // A driver reset or update: every view builds again on a new device. A second failure
    // there is reported like the first.
    console.warn("visualizer: GPU device lost", info.reason, info.message);
    core = null;
    views.forEach((v) => v.restart());
  });
  const format = navigator.gpu.getPreferredCanvasFormat();
  const module = device.createShaderModule({ code: SHADER });
  const add: GPUBlendComponent = { srcFactor: "one", dstFactor: "one", operation: "add" };
  return {
    device,
    format,
    warp: device.createRenderPipeline({
      layout: "auto",
      vertex: { module, entryPoint: "full_vs" },
      fragment: { module, entryPoint: "warp_fs", targets: [{ format: FEEDBACK_FORMAT }] },
    }),
    wave: device.createRenderPipeline({
      layout: "auto",
      vertex: { module, entryPoint: "wave_vs" },
      fragment: {
        module,
        entryPoint: "wave_fs",
        targets: [{ format: FEEDBACK_FORMAT, blend: { color: add, alpha: add } }],
      },
      primitive: { topology: "triangle-strip" },
    }),
    present: device.createRenderPipeline({
      layout: "auto",
      vertex: { module, entryPoint: "full_vs" },
      fragment: { module, entryPoint: "present_fs", targets: [{ format }] },
    }),
    sampler: device.createSampler({
      magFilter: "linear",
      minFilter: "linear",
      addressModeU: "mirror-repeat",
      addressModeV: "mirror-repeat",
    }),
  };
}

// ---- a view ------------------------------------------------------------------

/**
 * What the picture is laid over.
 * - `pane`: light over a dark theme's pane; on a light theme, a dark screen of its own, since
 *   light over light washes out to pastels.
 * - `behind`: always light over whatever is beneath (the window's own background).
 * - `screen`: always on black (the visualizer's own window).
 */
export type Backdrop = "pane" | "behind" | "screen";

/** The uniform block; see `struct U` in the shader. */
const U_FLOATS = 24;

interface Target {
  c: Core;
  context: GPUCanvasContext;
  uniforms: GPUBuffer;
  waveBuf: GPUBuffer;
  waveBind: GPUBindGroup;
  frames: { size: [number, number]; tex: GPUTexture[]; warpBind: GPUBindGroup[]; presentBind: GPUBindGroup[] } | null;
  /** Which of the two textures holds the last frame. */
  cur: number;
}

export class VisualizerView {
  private shown = false;
  private raf = 0;
  private stopTimer: ReturnType<typeof setTimeout> | null = null;
  private failed = false;
  private target: Target | null = null;
  private readonly scenes = new Scenes();
  private readonly u = new Float32Array(U_FLOATS);
  private lastNow = 0;
  private waveHue = Math.random();
  private themeCheckedAt = -Infinity;
  private lightTheme = false;

  /**
   * `stage` gets the class `on` while showing, for the fade; `message` takes the reason if
   * WebGPU fails.
   */
  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly stage: HTMLElement,
    private readonly message: HTMLElement,
    private readonly backdrop: Backdrop
  ) {
    views.add(this);
  }

  /** Show the picture, or fade it out. Idempotent. */
  setShown(on: boolean) {
    if (on === this.shown) return;
    this.shown = on;
    this.stage.classList.toggle("on", on);
    if (on) {
      if (this.stopTimer !== null) {
        clearTimeout(this.stopTimer);
        this.stopTimer = null;
      }
      void this.start();
    } else {
      unsubscribe();
      this.stopTimer = setTimeout(() => {
        this.stopTimer = null;
        cancelAnimationFrame(this.raf);
        this.raf = 0;
      }, FADE_MS);
    }
  }

  /** Build again after the GPU device was lost. */
  restart() {
    cancelAnimationFrame(this.raf);
    this.raf = 0;
    this.target = null;
    if (this.shown) void this.build();
  }

  private async start() {
    if (this.failed) return;
    try {
      await subscribe();
    } catch (e) {
      this.fail(`The visualizer couldn't start: ${e}`);
      return;
    }
    await this.build();
  }

  private async build() {
    try {
      if (!this.target) {
        core ??= initCore();
        const c = await core;
        const context = this.canvas.getContext("webgpu");
        if (!context) throw new Error("the canvas has no WebGPU context");
        // Premultiplied: the picture is light laid over what is beneath the canvas.
        context.configure({ device: c.device, format: c.format, alphaMode: "premultiplied" });
        const uniforms = c.device.createBuffer({
          size: U_FLOATS * 4,
          usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
        });
        const waveBuf = c.device.createBuffer({
          size: WAVE * 4,
          usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST,
        });
        const waveBind = c.device.createBindGroup({
          layout: c.wave.getBindGroupLayout(0),
          entries: [
            { binding: 0, resource: { buffer: uniforms } },
            { binding: 3, resource: { buffer: waveBuf } },
          ],
        });
        this.target = { c, context, uniforms, waveBuf, waveBind, frames: null, cur: 0 };
      }
    } catch (e) {
      this.fail(`The visualizer needs WebGPU, and the graphics driver wouldn't provide it (${e}).`);
      return;
    }
    const t = this.target;
    if (this.shown && this.raf === 0) this.raf = requestAnimationFrame((now) => this.draw(t, now));
  }

  private fail(why: string) {
    console.error(why);
    this.failed = true;
    this.message.textContent = why;
    this.message.hidden = false;
    this.canvas.hidden = true;
    if (this.shown) unsubscribe();
    this.shown = false;
  }

  /** The feedback textures at `w` × `h`, starting black. */
  private sizeFrames(t: Target, w: number, h: number) {
    if (t.frames && t.frames.size[0] === w && t.frames.size[1] === h) return t.frames;
    t.frames?.tex.forEach((x) => x.destroy());
    const { device } = t.c;
    const tex = [0, 1].map(() =>
      device.createTexture({
        size: [w, h],
        format: FEEDBACK_FORMAT,
        usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.TEXTURE_BINDING,
      })
    );
    const bind = (pipeline: GPURenderPipeline, x: GPUTexture) =>
      device.createBindGroup({
        layout: pipeline.getBindGroupLayout(0),
        entries: [
          { binding: 0, resource: { buffer: t.uniforms } },
          { binding: 1, resource: t.c.sampler },
          { binding: 2, resource: x.createView() },
        ],
      });
    t.frames = {
      size: [w, h],
      tex,
      warpBind: tex.map((x) => bind(t.c.warp, x)),
      presentBind: tex.map((x) => bind(t.c.present, x)),
    };
    t.cur = 0;
    return t.frames;
  }

  private draw(t: Target, now: number) {
    this.raf = requestAnimationFrame((n) => this.draw(t, n));
    const dt = Math.min(0.1, Math.max(0, (now - (this.lastNow || now)) / 1000));
    this.lastNow = now;

    const canvas = this.canvas;
    const dpr = window.devicePixelRatio || 1;
    const w = Math.max(1, Math.round(canvas.clientWidth * dpr));
    const h = Math.max(1, Math.round(canvas.clientHeight * dpr));
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
    const frames = this.sizeFrames(t, w, h);
    // The theme changes rarely; reading styles every frame would be waste.
    if (this.backdrop === "pane" && now - this.themeCheckedAt > 500) {
      this.themeCheckedAt = now;
      const bg = rgb(getComputedStyle(document.body).backgroundColor);
      this.lightTheme = 0.2126 * bg[0] + 0.7152 * bg[1] + 0.0722 * bg[2] > 0.5;
    }
    const opaque = this.backdrop === "screen" || (this.backdrop === "pane" && this.lightTheme);

    const time = now / 1000;
    const bass = mean(latest, 0, 16);
    const mid = mean(latest, 16, 40);
    const treb = mean(latest, 40, BANDS);
    const pulse = latest[BANDS + 2];
    const s = this.scenes.now(time, pulse);
    this.waveHue = (this.waveHue + dt * (0.04 + 0.4 * pulse)) % 1;

    // prettier-ignore
    this.u.set([
      w, h, time, dt * 60,
      bass, mid, treb, pulse,
      s.zoom, s.rot, s.warp, s.warpFreq,
      s.decay, s.hueShift, s.waveMode, s.waveSize,
      s.cx, s.cy, this.waveHue, latest[BANDS],
      opaque ? 1 : 0, s.echo, 0, 0,
    ]);
    const { device } = t.c;
    device.queue.writeBuffer(t.uniforms, 0, this.u);
    device.queue.writeBuffer(t.waveBuf, 0, latest, BANDS + 3, WAVE);

    const next = 1 - t.cur;
    const encoder = device.createCommandEncoder();
    const feedback = encoder.beginRenderPass({
      colorAttachments: [
        {
          view: frames.tex[next].createView(),
          clearValue: { r: 0, g: 0, b: 0, a: 1 },
          loadOp: "clear",
          storeOp: "store",
        },
      ],
    });
    feedback.setPipeline(t.c.warp);
    feedback.setBindGroup(0, frames.warpBind[t.cur]);
    feedback.draw(3);
    feedback.setPipeline(t.c.wave);
    feedback.setBindGroup(0, t.waveBind);
    // Two vertices per point, and one more point to close a ring.
    feedback.draw(2 * (WAVE + 1));
    feedback.end();

    const screen = encoder.beginRenderPass({
      colorAttachments: [
        {
          view: t.context.getCurrentTexture().createView(),
          clearValue: { r: 0, g: 0, b: 0, a: 0 },
          loadOp: "clear",
          storeOp: "store",
        },
      ],
    });
    screen.setPipeline(t.c.present);
    screen.setBindGroup(0, frames.presentBind[next]);
    screen.draw(3);
    screen.end();
    device.queue.submit([encoder.finish()]);
    t.cur = next;
  }
}

const mean = (a: Float32Array, from: number, to: number) => {
  let sum = 0;
  for (let i = from; i < to; i++) sum += a[i];
  return sum / (to - from);
};

/** Any CSS colour as 0..1 RGB, by letting a 2D context parse it. */
const parser = document.createElement("canvas").getContext("2d")!;
function rgb(css: string): [number, number, number] {
  parser.fillStyle = "#000";
  parser.fillStyle = css.trim();
  const v = parser.fillStyle;
  if (v.startsWith("#")) {
    const n = parseInt(v.slice(1), 16);
    return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
  }
  const m = v.match(/[\d.]+/g)?.map(Number) ?? [0, 0, 0];
  return [m[0] / 255, m[1] / 255, m[2] / 255];
}

const SHADER = /* wgsl */ `
struct U {
  res: vec2f,
  time: f32,
  dt60: f32,
  bass: f32,
  mid: f32,
  treb: f32,
  pulse: f32,
  zoom: f32,
  rot: f32,
  warp: f32,
  warp_freq: f32,
  decay: f32,
  hue_shift: f32,
  wave_mode: f32,
  wave_size: f32,
  center: vec2f,
  wave_hue: f32,
  level: f32,
  opaque: f32,
  echo: f32,
  pad0: f32,
  pad1: f32,
};
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var prev: texture_2d<f32>;
@group(0) @binding(3) var<storage, read> wave: array<f32, ${WAVE}>;

const TAU = 6.2831853;
const N = ${WAVE};

// Rotate a colour's hue about the grey axis.
fn hue(c: vec3f, a: f32) -> vec3f {
  let k = vec3f(0.57735);
  let ca = cos(a);
  return c * ca + cross(k, c) * sin(a) + k * dot(k, c) * (1.0 - ca);
}

fn hsv(h: f32, s: f32, v: f32) -> vec3f {
  let k = clamp(abs(fract(h + vec3f(0.0, 2.0 / 3.0, 1.0 / 3.0)) * 6.0 - 3.0) - 1.0, vec3f(0.0), vec3f(1.0));
  return v * mix(vec3f(1.0), k, s);
}

fn rotate(p: vec2f, a: f32) -> vec2f {
  let c = cos(a);
  let s = sin(a);
  return vec2f(p.x * c - p.y * s, p.x * s + p.y * c);
}

@vertex
fn full_vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
  var p = array(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
  return vec4f(p[i], 0.0, 1.0);
}

// The last frame, moved on by one step: zoomed about the centre (harder on a beat), turned,
// rippled, its colours drifting round the wheel, and fading.
@fragment
fn warp_fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
  let aspect = u.res.x / u.res.y;
  var p = (pos.xy / u.res - u.center) * vec2f(aspect, 1.0);
  let r = length(p);
  p = p / pow(u.zoom + 0.012 * u.pulse + 0.004 * r * u.bass, u.dt60);
  p = rotate(p, (u.rot + 0.01 * u.pulse * sign(u.rot)) * u.dt60);
  let w = u.warp * (0.5 + 1.5 * u.treb) * u.dt60;
  p += w * vec2f(sin(p.y * u.warp_freq + u.time * 1.3), cos(p.x * u.warp_freq * 1.1 - u.time * 0.9));
  let at = p / vec2f(aspect, 1.0) + u.center;
  var c = textureSampleLevel(prev, samp, at, 0.0).rgb;
  c = hue(c, u.hue_shift * u.dt60);
  c = max(c * pow(u.decay, u.dt60) - vec3f(0.0015 * u.dt60), vec3f(0.0));
  return vec4f(c, 1.0);
}

struct WaveOut {
  @builtin(position) pos: vec4f,
  @location(0) side: f32,
  @location(1) along: f32,
};

// Point i of the wave, in units of the pane's height, centred, y up.
fn wave_at(i: i32) -> vec2f {
  let aspect = u.res.x / u.res.y;
  let k = clamp(i, 0, N - 1);
  let f = f32(i) / f32(N);
  let size = u.wave_size;
  if (u.wave_mode < 0.5) {
    let x = (f32(k) / f32(N - 1) - 0.5) * 0.9 * aspect;
    return vec2f(x, wave[k] * 0.22 * size);
  }
  let j = ((i % N) + N) % N;
  if (u.wave_mode < 1.5) {
    let r = (0.26 + wave[j] * 0.07) * size;
    return rotate(vec2f(r, 0.0), f * TAU + u.time * 0.2);
  }
  let r = (0.04 + 0.36 * f32(k) / f32(N - 1) + wave[k] * 0.05) * size;
  return rotate(vec2f(r, 0.0), f * TAU * 2.0 - u.time * 0.3);
}

@vertex
fn wave_vs(@builtin(vertex_index) v: u32) -> WaveOut {
  let i = i32(v / 2u);
  let side = select(-1.0, 1.0, (v & 1u) == 1u);
  let p = wave_at(i);
  let tangent = wave_at(i + 1) - wave_at(i - 1);
  let normal = normalize(vec2f(-tangent.y, tangent.x) + vec2f(1e-6, 0.0));
  let thick = (0.0035 + 0.006 * u.bass + 0.004 * u.pulse) * u.wave_size;
  let q = p + normal * side * thick;
  let aspect = u.res.x / u.res.y;
  let c = q + (u.center - 0.5) * vec2f(2.0 * aspect, -2.0) * 0.5;
  var out: WaveOut;
  out.pos = vec4f(c.x * 2.0 / aspect, c.y * 2.0, 0.0, 1.0);
  out.side = side;
  out.along = f32(i) / f32(N);
  return out;
}

@fragment
fn wave_fs(in: WaveOut) -> @location(0) vec4f {
  let edge = 1.0 - in.side * in.side;
  let bright = (0.35 + 0.9 * u.level + 0.6 * u.pulse) * edge;
  let col = hsv(fract(u.wave_hue + in.along * 0.35), 0.8, 1.0) * bright;
  return vec4f(col, 1.0);
}

// The feedback image onto the canvas, with the echo laid over it, highlights rolled off rather
// than clipped. Either opaque, on black, or light over what is beneath, which shows through
// where the picture is dark (see Backdrop).
@fragment
fn present_fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
  let uv = pos.xy / u.res;
  var c = textureSampleLevel(prev, samp, uv, 0.0).rgb;
  if (u.echo > 0.0) {
    let e = (vec2f(1.0) - uv - 0.5) / 1.35 + 0.5;
    c += textureSampleLevel(prev, samp, e, 0.0).rgb * u.echo;
  }
  c = 1.0 - exp(-c * 1.4);
  if (u.opaque > 0.5) {
    return vec4f(c, 1.0);
  }
  let a = clamp(max(c.r, max(c.g, c.b)), 0.0, 1.0);
  return vec4f(c, a);
}
`;
