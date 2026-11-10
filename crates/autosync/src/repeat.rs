//! Repeats the speech model missed: a chorus heard once can place the copies it didn't hear.
//!
//! The model often hears a chorus once and then nothing of its later copies, or nothing it can
//! use, once the mix gets thick (ILUKA's "Wings": chorus 1 heard, choruses 2 and 3 not a word).
//! But a repeated block is *sung over the same music*. So where the lyrics say a block of lines
//! repeats, one copy was heard well and another hardly at all, the music of the heard copy is
//! matched against the stretch of the song where the missing copy must be. Where it clearly
//! matches, the missing copy gets the heard copy's line times, shifted.
//!
//! - [`candidates`]: which blocks repeat, and which copies are missing. Text and alignment only.
//! - [`Features`]: what the music sounds like, frame by frame, from 16-bit PCM.
//! - [`find`]: where each missing copy is, or nothing where the music doesn't say clearly.
//! - [`crate::align::Alignment::anchor_repeats`] puts the result into an alignment.
//!
//! See `plans/lyric-sections.md`.

use crate::align::Alignment;
use crate::text::words;
use rustfft::{num_complex::Complex32, FftPlanner};

/// A copy needs at least this share of its words heard to be the one the others are placed from.
const REFERENCE_SHARE: f64 = 0.6;
/// A copy with less than this share heard is treated as missed.
const MISSED_SHARE: f64 = 0.3;
/// A block is at least this many sung lines. One line repeating says too little about the music.
const MIN_LINES: usize = 2;

/// The match must score at least this (cosine, -1 to 1) over the whole block…
const MIN_SCORE: f64 = 0.5;
/// …stand this far above the typical score in its window…
const MIN_OVER_MEDIAN: f64 = 0.15;
/// …and this far above the best placement at least a second away from it.
const MIN_OVER_NEXT: f64 = 0.05;

/// The template runs from just before the block's first line to a little after its last line
/// starts, so the last line's music counts too.
const LEAD: f64 = 0.5;
const TAIL: f64 = 3.0;
/// Room left between a placed copy and the anchored lines either side of it.
const MARGIN: f64 = 0.3;

/// A copy of a repeated block that was missed, found in the music. Line indices are into the
/// lines the alignment was made from; `len` counts sung lines, blank lines and labels aside.
#[derive(Debug, Clone, PartialEq)]
pub struct Repeat {
    /// First line of the copy that was heard.
    pub reference: usize,
    /// First line of the copy that was missed.
    pub copy: usize,
    pub len: usize,
    /// Seconds from the heard copy to the missed one.
    pub offset: f64,
}

/// A missed copy and the heard one to place it from, before the music has been consulted.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub reference: usize,
    pub copy: usize,
    pub len: usize,
}

/// Lines with words (no blanks, no labels), as indices into `aligned.lines`.
fn sung(aligned: &Alignment) -> Vec<usize> {
    (0..aligned.lines.len())
        .filter(|&i| aligned.lines[i].words > 0)
        .collect()
}

/// Every missed copy of a repeated block that has a well-heard copy to be placed from.
pub fn candidates(lines: &[&str], aligned: &Alignment) -> Vec<Candidate> {
    let sung = sung(aligned);
    let key: Vec<String> = sung.iter().map(|&i| words(lines[i]).join(" ")).collect();
    let share = |p: usize, len: usize| {
        let (w, m) = sung[p..p + len].iter().fold((0, 0), |(w, m), &i| {
            (w + aligned.lines[i].words, m + aligned.lines[i].matched)
        });
        m as f64 / w.max(1) as f64
    };

    // The best reference for each missed copy, by the share of it heard.
    let mut best: Vec<(Candidate, f64)> = Vec::new();
    for p in 0..key.len() {
        for q in p + 1..key.len() {
            // Maximal runs only: start where the line before doesn't also match.
            if key[p] != key[q] || (p > 0 && key[p - 1] == key[q - 1]) {
                continue;
            }
            let mut len = 0;
            while q + len < key.len() && p + len < q && key[p + len] == key[q + len] {
                len += 1;
            }
            if len < MIN_LINES {
                continue;
            }
            let (sp, sq) = (share(p, len), share(q, len));
            let (reference, copy, s) = if sp >= REFERENCE_SHARE && sq < MISSED_SHARE {
                (p, q, sp)
            } else if sq >= REFERENCE_SHARE && sp < MISSED_SHARE {
                (q, p, sq)
            } else {
                continue;
            };
            let c = Candidate {
                reference: sung[reference],
                copy: sung[copy],
                len,
            };
            match best.iter_mut().find(|(b, _)| b.copy == c.copy) {
                Some(b) if b.1 >= s => {}
                Some(b) => *b = (c, s),
                None => best.push((c, s)),
            }
        }
    }
    best.into_iter().map(|(c, _)| c).collect()
}

/// What the music sounds like, frame by frame: pitch classes (chroma) for the harmony, and
/// energy in log-spaced bands for the texture, each normalised over the track so that what
/// varies counts rather than what is always there.
pub struct Features {
    /// Frames per second.
    fps: f64,
    frames: Vec<Vec<f32>>,
}

const FFT: usize = 2048;
const CHROMA: (f32, f32) = (80.0, 4000.0);
const BANDS: usize = 24;
const BAND_RANGE: (f32, f32) = (150.0, 7000.0);

impl Features {
    /// From 16-bit mono PCM at `sample_rate`, one frame every 20 ms.
    pub fn from_pcm16(pcm: &[i16], sample_rate: u32) -> Features {
        let sr = sample_rate as f32;
        let hop = (sample_rate / 50).max(1) as usize;
        let window: Vec<f32> = (0..FFT)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FFT as f32).cos())
            .collect();
        let fft = FftPlanner::<f32>::new().plan_fft_forward(FFT);

        // Which feature each FFT bin adds its power to.
        let bin_hz = |k: usize| k as f32 * sr / FFT as f32;
        let edges: Vec<f32> = (0..=BANDS)
            .map(|b| BAND_RANGE.0 * (BAND_RANGE.1 / BAND_RANGE.0).powf(b as f32 / BANDS as f32))
            .collect();
        let pitch_class = |hz: f32| ((12.0 * (hz / 440.0).log2()).round() as i32).rem_euclid(12);

        let mut chroma: Vec<[f32; 12]> = Vec::new();
        let mut bands: Vec<[f32; BANDS]> = Vec::new();
        let mut buf = vec![Complex32::new(0.0, 0.0); FFT];
        let mut start = 0;
        while start + FFT <= pcm.len() {
            for (i, b) in buf.iter_mut().enumerate() {
                *b = Complex32::new(pcm[start + i] as f32 / 32768.0 * window[i], 0.0);
            }
            fft.process(&mut buf);
            let (mut c, mut e) = ([0f32; 12], [0f32; BANDS]);
            for (k, v) in buf.iter().enumerate().take(FFT / 2 + 1).skip(1) {
                let (hz, power) = (bin_hz(k), v.norm_sqr());
                if (CHROMA.0..=CHROMA.1).contains(&hz) {
                    c[pitch_class(hz) as usize] += power;
                }
                if let Some(b) = edges.windows(2).position(|w| hz >= w[0] && hz < w[1]) {
                    e[b] += power;
                }
            }
            chroma.push(c.map(|p| (p * 100.0).ln_1p()));
            bands.push(e.map(|p| (p * 100.0).ln_1p()));
            start += hop;
        }

        let chroma = unit_rows(zscore(chroma));
        let bands = unit_rows(zscore(bands));
        let frames = chroma
            .into_iter()
            .zip(bands)
            .map(|(c, b)| {
                let mut f: Vec<f32> = c.into_iter().chain(b).collect();
                unit(&mut f);
                f
            })
            .collect();
        Features {
            fps: sample_rate as f64 / hop as f64,
            frames,
        }
    }

    fn frame(&self, t: f64) -> usize {
        ((t * self.fps).round().max(0.0) as usize).min(self.frames.len())
    }

    /// Mean similarity of the frames from `t0` with the template frames.
    fn score(&self, template: &[Vec<f32>], at: usize) -> f64 {
        let n = template.len();
        let sum: f32 = template
            .iter()
            .zip(&self.frames[at..at + n])
            .map(|(a, b)| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>())
            .sum();
        sum as f64 / n.max(1) as f64
    }
}

fn zscore<const N: usize>(rows: Vec<[f32; N]>) -> Vec<Vec<f32>> {
    let n = rows.len().max(1) as f32;
    let mut mean = [0f32; N];
    let mut var = [0f32; N];
    for r in &rows {
        for d in 0..N {
            mean[d] += r[d] / n;
        }
    }
    for r in &rows {
        for d in 0..N {
            var[d] += (r[d] - mean[d]).powi(2) / n;
        }
    }
    rows.iter()
        .map(|r| {
            (0..N)
                .map(|d| (r[d] - mean[d]) / (var[d].sqrt() + 1e-9))
                .collect()
        })
        .collect()
}

fn unit(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt() + 1e-9;
    v.iter_mut().for_each(|x| *x /= norm);
}

fn unit_rows(mut rows: Vec<Vec<f32>>) -> Vec<Vec<f32>> {
    rows.iter_mut().for_each(|r| unit(r));
    rows
}

/// Where each missed copy is, for those the music places clearly.
pub fn find(
    lines: &[&str],
    aligned: &Alignment,
    features: &Features,
    duration: f64,
) -> Vec<Repeat> {
    let sung = sung(aligned);
    let pos = |line: usize| sung.iter().position(|&i| i == line).unwrap_or(0);
    // A line's time only counts as known when it was read off words heard in it.
    let anchored = |i: usize| {
        let l = &aligned.lines[i];
        l.t.filter(|_| !l.interpolated && l.matched > 0)
    };
    let mut out = Vec::new();
    for c in candidates(lines, aligned) {
        let refs = &sung[pos(c.reference)..pos(c.reference) + c.len];
        let copy = &sung[pos(c.copy)..pos(c.copy) + c.len];
        let (Some(r0), Some(r1)) = (aligned.lines[refs[0]].t, aligned.lines[refs[c.len - 1]].t)
        else {
            continue;
        };
        // The copy fits between the anchored lines either side of it.
        let before = (0..copy[0]).rev().find_map(anchored).unwrap_or(0.0);
        let after = (copy[c.len - 1] + 1..aligned.lines.len())
            .find_map(anchored)
            .unwrap_or(duration);
        let (lo, hi) = (before + MARGIN, after - (r1 - r0) - MARGIN);
        if hi < lo {
            continue;
        }

        let (t0, t1) = (features.frame(r0 - LEAD), features.frame(r1 + TAIL));
        let template = &features.frames[t0..t1];
        if template.is_empty() {
            continue;
        }
        let last_start = features.frames.len().saturating_sub(template.len());
        let (a, b) = (
            features.frame(lo - LEAD),
            features.frame(hi - LEAD).min(last_start),
        );
        if b < a {
            continue;
        }
        let scores: Vec<f64> = (a..=b).map(|s| features.score(template, s)).collect();
        let (best_at, best) = scores
            .iter()
            .copied()
            .enumerate()
            .fold((0, f64::MIN), |m, (i, s)| if s > m.1 { (i, s) } else { m });
        let second = scores
            .iter()
            .enumerate()
            .filter(|(i, _)| (*i as f64 - best_at as f64).abs() > features.fps)
            .map(|(_, &s)| s)
            .fold(f64::MIN, f64::max);
        let mut sorted = scores.clone();
        sorted.sort_by(f64::total_cmp);
        let median = sorted[sorted.len() / 2];
        let clear = best >= MIN_SCORE
            && best - median >= MIN_OVER_MEDIAN
            && (second == f64::MIN || best - second >= MIN_OVER_NEXT);
        if !clear {
            continue;
        }
        let start = (a + best_at) as f64 / features.fps + LEAD;
        out.push(Repeat {
            reference: c.reference,
            copy: c.copy,
            len: c.len,
            offset: start - r0,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::align::align;
    use crate::Heard;

    /// Words one after another from `start`, `gap` seconds apart.
    fn sung_words(text: &str, start: f64, gap: f64) -> Vec<Heard> {
        text.split(' ')
            .enumerate()
            .map(|(i, w)| Heard {
                text: w.into(),
                start: start + i as f64 * gap,
                end: start + i as f64 * gap + gap * 0.8,
                p: 1.0,
            })
            .collect()
    }

    const LINES: [&str; 9] = [
        "walking down the empty road tonight",
        "nobody there to hold the light",
        "",
        "Chorus:",
        "we are the fire in the rain",
        "we are the ones who stay the same",
        "",
        "we are the fire in the rain",
        "we are the ones who stay the same",
    ];

    /// The verse and the first chorus heard; the second chorus not at all.
    fn heard() -> Vec<Heard> {
        let mut h = sung_words("walking down the empty road tonight", 5.0, 0.4);
        h.extend(sung_words("nobody there to hold the light", 8.0, 0.4));
        h.extend(sung_words("we are the fire in the rain", 12.0, 0.4));
        h.extend(sung_words("we are the ones who stay the same", 15.0, 0.4));
        h
    }

    #[test]
    fn a_missed_copy_with_a_heard_one_is_a_candidate() {
        let a = align(&LINES, &heard());
        assert_eq!(
            candidates(&LINES, &a),
            [Candidate {
                reference: 4,
                copy: 7,
                len: 2
            }]
        );
    }

    #[test]
    fn two_heard_copies_need_nothing() {
        let mut h = heard();
        h.extend(sung_words("we are the fire in the rain", 25.0, 0.4));
        h.extend(sung_words("we are the ones who stay the same", 28.0, 0.4));
        assert!(candidates(&LINES, &align(&LINES, &h)).is_empty());
    }

    /// A tone per second of "music": a block is a run of distinct pitches.
    fn tones(pattern: &[f32], sample_rate: u32) -> Vec<i16> {
        pattern
            .iter()
            .flat_map(|&hz| {
                (0..sample_rate).map(move |i| {
                    let t = i as f32 / sample_rate as f32;
                    let v = if hz > 0.0 {
                        (2.0 * std::f32::consts::PI * hz * t).sin()
                            + 0.5 * (2.0 * std::f32::consts::PI * hz * 2.0 * t).sin()
                    } else {
                        0.0
                    };
                    (v * 8000.0) as i16
                })
            })
            .collect()
    }

    #[test]
    fn the_music_places_the_missed_copy() {
        // 0-11 s verse, 11-19 s chorus, 19-24 s a break, 24-32 s the chorus again, then an outro.
        let verse = [
            220.0, 247.0, 262.0, 294.0, 330.0, 349.0, 392.0, 440.0, 494.0, 523.0, 587.0,
        ];
        let chorus = [330.0, 392.0, 523.0, 659.0, 587.0, 494.0, 440.0, 392.0];
        let brk = [175.0, 196.0, 185.0, 208.0, 165.0];
        let outro = [110.0, 123.0, 131.0, 147.0];
        let song: Vec<f32> = [&verse[..], &chorus, &brk, &chorus, &outro].concat();
        let features = Features::from_pcm16(&tones(&song, 16_000), 16_000);
        let a = align(&LINES, &heard());
        let found = find(&LINES, &a, &features, song.len() as f64);
        assert_eq!(found.len(), 1, "{found:?}");
        // The chorus came back 13 s later.
        assert!((found[0].offset - 13.0).abs() < 0.1, "{found:?}");

        let mut a = a;
        a.anchor_repeats(&found);
        let placed = a.placed(song.len() as f64);
        let (r, c) = (placed[4].unwrap(), placed[7].unwrap());
        assert!((c - r - 13.0).abs() < 0.1, "{placed:?}");
        assert_eq!(a.coverage(), 1.0, "the copy found counts as heard");
    }

    #[test]
    fn music_that_does_not_repeat_places_nothing() {
        let verse = [
            220.0, 247.0, 262.0, 294.0, 330.0, 349.0, 392.0, 440.0, 494.0, 523.0, 587.0,
        ];
        let chorus = [330.0, 392.0, 523.0, 659.0, 587.0, 494.0, 440.0, 392.0];
        let other = [
            185.0, 208.0, 233.0, 277.0, 311.0, 370.0, 415.0, 466.0, 554.0, 622.0, 740.0, 831.0,
            932.0, 1109.0, 1245.0, 1480.0, 1661.0,
        ];
        let song: Vec<f32> = [&verse[..], &chorus, &other].concat();
        let features = Features::from_pcm16(&tones(&song, 16_000), 16_000);
        let a = align(&LINES, &heard());
        assert!(find(&LINES, &a, &features, song.len() as f64).is_empty());
    }

    /// A real song: `AUTOSYNC_REAL_SONG` names a directory with `audio.pcm` (16 kHz mono s16le),
    /// `heard.json` (a stored autosync result) and `plain.txt`. Optional `expect.txt` holds the
    /// expected offsets, one per line. Run with `cargo test -p autosync -- --ignored real_song`.
    #[test]
    #[ignore]
    fn real_song() {
        let dir = std::path::PathBuf::from(std::env::var("AUTOSYNC_REAL_SONG").unwrap());
        let bytes = std::fs::read(dir.join("audio.pcm")).unwrap();
        let pcm: Vec<i16> = bytes
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("heard.json")).unwrap()).unwrap();
        let heard: Vec<Heard> = stored["heard"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| Heard {
                text: w[0].as_str().unwrap().into(),
                start: w[1].as_f64().unwrap(),
                end: w[2].as_f64().unwrap(),
                p: 1.0,
            })
            .collect();
        let plain = std::fs::read_to_string(dir.join("plain.txt")).unwrap();
        let lines: Vec<&str> = plain.lines().collect();
        let duration = pcm.len() as f64 / 16_000.0;

        let started = std::time::Instant::now();
        let features = Features::from_pcm16(&pcm, 16_000);
        let mut a = crate::align::align(&lines, &heard);
        let before = a.coverage();
        let found = find(&lines, &a, &features, duration);
        eprintln!("{:?} for features and search", started.elapsed());
        a.anchor_repeats(&found);
        eprintln!("coverage {before:.2} -> {:.2}", a.coverage());
        for r in &found {
            eprintln!("{:?} {:?} -> {:?}", r, lines[r.reference], lines[r.copy]);
        }
        for l in a.timed(&lines, duration) {
            eprintln!("{:7.2} {}", l.t, l.text);
        }
        if let Ok(expect) = std::fs::read_to_string(dir.join("expect.txt")) {
            let want: Vec<f64> = expect.lines().map(|l| l.trim().parse().unwrap()).collect();
            let got: Vec<f64> = found.iter().map(|r| r.offset).collect();
            assert_eq!(got.len(), want.len(), "{got:?}");
            for (g, w) in got.iter().zip(&want) {
                assert!((g - w).abs() < 0.15, "{got:?} vs {want:?}");
            }
        }
    }
}
