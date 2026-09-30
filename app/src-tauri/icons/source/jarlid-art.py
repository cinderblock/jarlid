#!/usr/bin/env python3
"""Draws the Jarlid app icon: a glass mason jar whose lid has popped off, with music notes
escaping (one still trapped inside, one breaking free).

The jar and lid are modelled in 3D, projected isometrically and shaded per face against one
light, and written out as plain SVG polygons, so the result is too dense to edit by hand. Edit
this file instead and re-run scripts/build-icons.py, which calls it before regenerating the
PNG/ICO/ICNS set.

Writes, next to this file:
  jarlid.svg        full art, used for 40 px and up
  jarlid-small.svg  heavier art for 16-32 px (the taskbar), where the fine glass detail would
                    blur away: bigger jar, thicker glass edges, a bigger note
"""
import math
from pathlib import Path

HERE = Path(__file__).resolve().parent

# --- projection and shading ---------------------------------------------------------------

EL = math.radians(35.264)  # true isometric elevation; horizontal circles become 1 : 0.577 ellipses
AZ = math.radians(45)
_L = (-0.5, 0.62, 0.6)     # light, in view space (x right, y up, z toward the viewer)
LIGHT = tuple(c / math.hypot(*_L) for c in _L)
HALF = tuple(c / math.hypot(LIGHT[0], LIGHT[1], LIGHT[2] + 1) for c in (LIGHT[0], LIGHT[1], LIGHT[2] + 1))


def to_view(p):
    """World (y up) -> view space: x right, y up, z toward the viewer."""
    x, y, z = p
    x1 = x * math.cos(AZ) + z * math.sin(AZ)
    z1 = -x * math.sin(AZ) + z * math.cos(AZ)
    return (x1, y * math.cos(EL) - z1 * math.sin(EL), y * math.sin(EL) + z1 * math.cos(EL))


def shade(rgb, n, amb=0.42, dif=0.62, spec=0.55, shin=24):
    nv = to_view(n)
    d = max(0.0, sum(a * b for a, b in zip(nv, LIGHT)))
    s = max(0.0, sum(a * b for a, b in zip(nv, HALF))) ** shin * spec
    return '#%02x%02x%02x' % tuple(max(0, min(255, round(c * (amb + dif * d) + 255 * s))) for c in rgb)


class Mesh:
    """Flat-shaded polygons, painted back to front. Screen origin is the model's (0, 0, 0)."""

    def __init__(self, xf=lambda p: p, nxf=lambda n: n):
        self.faces, self.xf, self.nxf = [], xf, nxf

    def quad(self, pts, rgb, n, **light):
        n = self.nxf(n)
        if to_view(n)[2] <= 0.001:
            return
        sp = [to_view(self.xf(p)) for p in pts]
        depth = sum(p[2] for p in sp) / len(sp)
        c = shade(rgb, n, **light)
        pts = ' '.join(f'{x:.1f},{-y:.1f}' for x, y, _ in sp)
        # A hairline stroke in the fill colour closes the anti-aliasing seams between faces.
        self.faces.append((depth, f'<polygon points="{pts}" fill="{c}" stroke="{c}" stroke-width="1.2" stroke-linejoin="round"/>'))

    def revolve(self, profile, rgb, steps=96, knurl=False, **light):
        """Surface of revolution from a (radius, height) profile listed bottom to top."""
        for (r0, y0), (r1, y1) in zip(profile, profile[1:]):
            ln = math.hypot(r1 - r0, y1 - y0) or 1
            nr, ny = (y1 - y0) / ln, -(r1 - r0) / ln
            for i in range(steps):
                t0, t1 = 2 * math.pi * i / steps, 2 * math.pi * (i + 1) / steps
                tm = (t0 + t1) / 2
                col = tuple(c * 0.7 for c in rgb) if knurl and i % 2 else rgb
                self.quad([(r0 * math.cos(t0), y0, r0 * math.sin(t0)), (r0 * math.cos(t1), y0, r0 * math.sin(t1)),
                           (r1 * math.cos(t1), y1, r1 * math.sin(t1)), (r1 * math.cos(t0), y1, r1 * math.sin(t0))],
                          col, (nr * math.cos(tm), ny, nr * math.sin(tm)), **light)

    def disk(self, r, y, rgb, up=True, steps=96, **light):
        pts = [(r * math.cos(2 * math.pi * i / steps), y, r * math.sin(2 * math.pi * i / steps)) for i in range(steps)]
        self.quad(pts, rgb, (0, 1 if up else -1, 0), **light)

    def svg(self):
        return '\n'.join(s for _, s in sorted(self.faces, key=lambda f: f[0]))


def screen(p):
    x, y, _ = to_view(p)
    return x, -y


def ring_path(y, r, front=True, n=64):
    """Front (or back) half of the horizontal circle of radius r at height y."""
    # The view-side depth of a point at angle a is r * sin(a - AZ): the front is [AZ, AZ + pi].
    pts = [screen((r * math.cos(a), y, r * math.sin(a)))
           for a in (AZ + math.pi * i / n + (0 if front else math.pi) for i in range(n + 1))]
    return 'M ' + ' L '.join(f'{x:.1f},{y:.1f}' for x, y in pts)


# --- the jar ------------------------------------------------------------------------------

R, BODY_TOP, NECK_R, MOUTH_R = 182, 330, 124, 134
SHOULDER_TOP = BODY_TOP + 64
NECK_TOP = SHOULDER_TOP + 78


def jar_profile():
    p = [(R, 24), (R, BODY_TOP)]
    for k in range(1, 11):  # shoulder: a quarter ellipse up and in to the neck
        a = math.pi / 2 * k / 10
        p.append((NECK_R + (R - NECK_R) * math.cos(a), BODY_TOP + 64 * math.sin(a)))
    b, t = SHOULDER_TOP, NECK_R + 8  # two screw threads, then the lip
    p += [(NECK_R, b + 8), (t, b + 16), (t, b + 24), (NECK_R, b + 32), (t, b + 40), (t, b + 48),
          (NECK_R, b + 56), (NECK_R, b + 64), (MOUTH_R, b + 70), (MOUTH_R, NECK_TOP)]
    return p


def silhouette():
    """Outline of the glass: up the left side, over the mouth, down the right, round the base."""
    prof = jar_profile()
    left = [(-r, -y * math.cos(EL)) for r, y in prof]
    right = [(r, -y * math.cos(EL)) for r, y in prof]
    d = f'M {left[0][0]:.1f},{left[0][1]:.1f} ' + ' '.join(f'L {x:.1f},{y:.1f}' for x, y in left[1:])
    d += f' A {MOUTH_R} {MOUTH_R * math.sin(EL):.1f} 0 0 1 {MOUTH_R:.1f},{-NECK_TOP * math.cos(EL):.1f} '
    d += ' '.join(f'L {x:.1f},{y:.1f}' for x, y in right[::-1][1:])
    d += f' A {R} {R * math.sin(EL):.1f} 0 0 1 {left[0][0]:.1f},{left[0][1]:.1f} Z'
    return d


def glass(uid, contents, bold=False):
    """The jar at the origin, with `contents` drawn between its back and front walls."""
    sil = silhouette()
    edge = 24 if bold else 7
    out = [f'<ellipse cx="0" cy="{R * math.sin(EL) - 10:.1f}" rx="{R + 70}" ry="{R * math.sin(EL) + 40:.1f}" fill="url(#shadow)"/>',
           f'<clipPath id="{uid}"><path d="{sil}"/></clipPath>',
           f'<path d="{sil}" fill="url(#glassBack)"/>']
    if not bold:  # back halves of the rings, seen through the glass
        for y, r in ((24, R), (BODY_TOP, R), (NECK_TOP, MOUTH_R)):
            out.append(f'<path d="{ring_path(y, r, front=False)}" fill="none" stroke="#8fd0ff" stroke-width="5" opacity="0.35"/>')
    out.append(contents)
    out.append(f'<path d="{sil}" fill="url(#{"glassFrontBold" if bold else "glassFront"})"/>')
    rings = ([(SHOULDER_TOP + 20, NECK_R + 8, 16, 0.7)] if bold else
             [(40, R, 7, 0.45), (BODY_TOP, R, 6, 0.35), (SHOULDER_TOP, NECK_R, 5, 0.5),
              (SHOULDER_TOP + 20, NECK_R + 8, 6, 0.6), (SHOULDER_TOP + 44, NECK_R + 8, 6, 0.6)])
    for y, r, w, o in rings:
        out.append(f'<path d="{ring_path(y, r)}" fill="none" stroke="#cfeaff" stroke-width="{w}" opacity="{o}"/>')
    streaks = [(-136, 30, 250, 60 if bold else 30, 0.45)] + ([] if bold else [(-98, 60, 220, 10, 0.3), (156, 40, 230, 8, 0.18)])
    out.append(f'<g clip-path="url(#{uid})">' + ''.join(
        f'<path d="M {x} {-y0} L {x} {-y1}" stroke="#fff" stroke-width="{w}" stroke-linecap="round" opacity="{o}"/>'
        for x, y0, y1, w, o in streaks) + '</g>')
    out.append(f'<path d="{sil}" fill="none" stroke="#a6dcff" stroke-width="{edge}" stroke-linejoin="round"/>')
    my = -NECK_TOP * math.cos(EL)
    out.append(f'<ellipse cx="0" cy="{my:.1f}" rx="{MOUTH_R}" ry="{MOUTH_R * math.sin(EL):.1f}" fill="none" '
               f'stroke="#dff2ff" stroke-width="{18 if bold else 9}"/>')
    return '\n'.join(out)


# --- the lid ------------------------------------------------------------------------------

def lid(tilt_deg):
    """A two-piece mason lid (knurled band, flat disc), centred on the origin and tipped
    tilt_deg about the world z axis so it reads as flipped off the jar."""
    a = math.radians(tilt_deg)
    rot = lambda p: (p[0] * math.cos(a) - p[1] * math.sin(a), p[0] * math.sin(a) + p[1] * math.cos(a), p[2])
    m = Mesh(xf=rot, nxf=rot)
    rl, h = 146, 64
    band = (196, 204, 216)
    m.revolve([(rl, 0), (rl, h - 10)], band, steps=120, knurl=True)
    m.revolve([(rl, h - 10), (rl - 4, h - 3), (rl - 12, h)], band, spec=0.2)
    m.revolve([(rl - 12, h), (rl - 22, h - 6), (rl - 30, h - 6)], (176, 186, 200))
    m.disk(rl - 30, h - 6, (200, 208, 220), amb=0.52, dif=0.38, spec=0.1)
    m.disk(rl - 62, h - 4, (212, 219, 230), amb=0.52, dif=0.38, spec=0.1)  # the raised centre panel
    m.revolve([(rl - 12, 0), (rl, 0)], (120, 128, 140))
    m.disk(rl - 12, 2, (90, 98, 110), up=False)
    return m.svg()


# --- notes --------------------------------------------------------------------------------

NOTE_GLYPHS = '''
 <g id="n8">
  <ellipse cx="0" cy="0" rx="28" ry="20" transform="rotate(-24)"/>
  <rect x="16" y="-108" width="11" height="106"/>
  <path d="M 16 -108 C 34 -84 70 -72 62 -30 C 58 -52 44 -62 27 -66 L 27 -108 Z"/>
 </g>
 <g id="n16">
  <ellipse cx="0" cy="0" rx="28" ry="20" transform="rotate(-24)"/>
  <ellipse cx="86" cy="-20" rx="28" ry="20" transform="rotate(-24 86 -20)"/>
  <rect x="16" y="-108" width="11" height="106"/>
  <rect x="102" y="-128" width="11" height="106"/>
  <path d="M 16 -108 L 113 -128 L 113 -100 L 16 -80 Z"/>
 </g>'''


def note(glyph, x, y, s, rot):
    """An extruded note: many thin darker copies stepping down-right, then the lit face. Steps
    of well under a pixel keep the sides smooth rather than stair-stepped."""
    depth = 20 * s                     # px of extrusion
    steps = max(4, math.ceil(depth / 0.6))
    ux, uy = 0.82, 0.57                # extrusion direction (away from the light)
    parts = []
    for i in range(steps, 0, -1):
        k = depth * i / steps
        col = '#a4521a' if i > steps * 0.25 else '#c86a20'
        parts.append(f'<use href="#{glyph}" fill="{col}" transform="translate({x + k * ux:.2f} {y + k * uy:.2f}) rotate({rot}) scale({s})"/>')
    parts.append(f'<use href="#{glyph}" fill="url(#noteFace)" transform="translate({x} {y}) rotate({rot}) scale({s})"/>')
    return ''.join(parts)


def trail(d, w):
    return (f'<path d="{d}" fill="none" stroke="#ffd98a" stroke-width="{w}" stroke-linecap="round" '
            f'stroke-dasharray="0.1 {w * 2.6:.0f}" opacity="0.75"/>')


# --- composition --------------------------------------------------------------------------

DEFS = '''
 <linearGradient id="tile" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#222c40"/><stop offset="1" stop-color="#0b0c10"/></linearGradient>
 <linearGradient id="glassFront" x1="0" y1="0" x2="1" y2="0">
  <stop offset="0" stop-color="#bfe4ff" stop-opacity="0.75"/><stop offset="0.12" stop-color="#8fd0ff" stop-opacity="0.3"/>
  <stop offset="0.5" stop-color="#6ec1ff" stop-opacity="0.1"/><stop offset="0.86" stop-color="#3f8fd6" stop-opacity="0.35"/>
  <stop offset="1" stop-color="#2a6fb4" stop-opacity="0.8"/></linearGradient>
 <linearGradient id="glassFrontBold" x1="0" y1="0" x2="1" y2="0">
  <stop offset="0" stop-color="#bfe4ff" stop-opacity="0.8"/><stop offset="0.2" stop-color="#8fd0ff" stop-opacity="0.45"/>
  <stop offset="0.6" stop-color="#6ec1ff" stop-opacity="0.3"/><stop offset="1" stop-color="#2a6fb4" stop-opacity="0.85"/></linearGradient>
 <linearGradient id="glassBack" x1="0" y1="0" x2="1" y2="0">
  <stop offset="0" stop-color="#0f3558" stop-opacity="0.55"/><stop offset="0.5" stop-color="#113c63" stop-opacity="0.35"/>
  <stop offset="1" stop-color="#0a2540" stop-opacity="0.6"/></linearGradient>
 <radialGradient id="shadow" cx="0.5" cy="0.5" r="0.5"><stop offset="0" stop-color="#000" stop-opacity="0.55"/><stop offset="1" stop-color="#000" stop-opacity="0"/></radialGradient>
 <linearGradient id="noteFace" x1="0" y1="0" x2="1" y2="1">
  <stop offset="0" stop-color="#ffe7a3"/><stop offset="0.5" stop-color="#ffc04d"/><stop offset="1" stop-color="#ff9b2e"/></linearGradient>
''' + NOTE_GLYPHS


def icon(tile, jar_at, jar_scale, trapped, lid_at, lid_scale, lid_tilt, lid_spin, free, trail_d=None, bold=False):
    """tile: the backing shape, as an SVG element with no fill. Everything is clipped to it, so
    the jar's floor shadow can't spill past the tile's edge."""
    jx, jy = jar_at
    lx, ly = lid_at
    body = [f'<g transform="translate({jx} {jy}) scale({jar_scale})">{glass("glass", note(*trapped), bold)}</g>',
            f'<g transform="translate({lx} {ly}) rotate({lid_spin}) scale({lid_scale})">{lid(lid_tilt)}</g>']
    if trail_d:
        body.append(trail(trail_d, 12))
    body.append(note(*free))
    return (f'<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 1024 1024">\n'
            f'<!-- Generated by jarlid-art.py. Edit that, not this. -->\n'
            f'<defs>{DEFS} <clipPath id="tileClip">{tile}</clipPath>\n</defs>\n'
            f'<g fill="url(#tile)">{tile}</g>\n<g clip-path="url(#tileClip)">\n'
            + '\n'.join(body) + '\n</g>\n</svg>\n')


def main():
    full = icon(tile='<rect x="24" y="24" width="976" height="976" rx="216"/>',
                jar_at=(528, 842), jar_scale=1.0, trapped=('n8', -16, -110, 1.05, 4),
                lid_at=(800, 300), lid_scale=1.0, lid_tilt=40, lid_spin=24,
                free=('n16', 262, 300, 1.6, -12),
                trail_d='M 520 400 C 516 360 500 336 470 318')
    small = icon(tile='<rect width="1024" height="1024" rx="176"/>',
                 jar_at=(560, 858), jar_scale=1.1, trapped=('n8', -18, -100, 1.3, 4),
                 lid_at=(830, 270), lid_scale=0.85, lid_tilt=40, lid_spin=24,
                 free=('n16', 150, 330, 2.05, -12), bold=True)
    (HERE / 'jarlid.svg').write_text(full, encoding='utf-8')
    (HERE / 'jarlid-small.svg').write_text(small, encoding='utf-8')


if __name__ == '__main__':
    main()
