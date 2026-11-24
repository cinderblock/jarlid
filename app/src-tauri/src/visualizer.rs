//! The visualizer's analysis: what is being heard, as a spectrum, sixty times a second.
//!
//! The page has no audio of its own (playback is all in Rust), so it cannot use Web Audio's
//! analyser. Instead this reads the player's [`audio::Scope`], the last moments of mixed audio
//! as heard, and sends the page a frame of numbers to draw on the GPU:
//!
//! - [`BANDS`] spectrum bands, log-spaced from 30 Hz to 16 kHz, each 0..1;
//! - the overall level, 0..1;
//! - the low end (kick and bass), 0..1;
//! - an onset pulse that jumps on a sharp rise in the low end and decays;
//! - [`WAVE`] points of the waveform itself, the last ~20 ms, scaled to the song's recent peak
//!   so a quiet song's wave is as tall as a loud one's.
//!
//! It runs only while a page has a visualizer on screen: [`visualizer_start`] starts a thread
//! for that webview, [`visualizer_stop`] (or the page going away) ends it. The main window and
//! the visualizer's own window each have one.
//!
//! The visualizer's own window ([`WINDOW`]) is opened and closed from its setting by [`apply`].
//!
//! Levels are relative to the song's own loudest recent moment rather than to full scale, so a
//! quiet acoustic track fills the picture as well as a loud master does. Band power is summed
//! over each band, so music, which is roughly pink, comes out roughly level across the bands.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustfft::num_complex::Complex32;
use rustfft::FftPlanner;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

use crate::native::NativeEngine;
use crate::settings;

/// The visualizer window's label, and its page (a second Vite entry).
pub const WINDOW: &str = "visualizer";
const WINDOW_PAGE: &str = "visualizer.html";

pub const BANDS: usize = 64;
const FFT_LEN: usize = 4096;
const LOW_HZ: f32 = 30.0;
const HIGH_HZ: f32 = 16_000.0;
const FRAME: Duration = Duration::from_micros(16_667);
/// How far below the recent peak a band reads as empty.
const RANGE_DB: f32 = 54.0;
/// How fast the recent peak forgets a loud moment, per second.
const PEAK_FALL_DB_PER_S: f32 = 3.0;
/// The quietest peak that counts as music, so silence is drawn as silence rather than having
/// its noise floor turned up to fill the picture.
const PEAK_FLOOR_DB: f32 = -70.0;
/// Bands that make up "the low end": up to about 150 Hz.
const LOW_BANDS: usize = 16;
/// Waveform points per frame, each the mean of [`WAVE_STEP`] samples.
pub const WAVE: usize = 256;
const WAVE_STEP: usize = 4;
/// How fast the waveform's scale forgets a loud moment, per second (as a factor).
const WAVE_PEAK_FALL_PER_S: f32 = 0.5;
/// Floats per frame sent to the page.
pub const FRAME_LEN: usize = BANDS + 3 + WAVE;

/// The running analysis threads, by the label of the webview each one feeds.
#[derive(Default)]
pub struct VisualizerCtl(Mutex<HashMap<String, Arc<AtomicBool>>>);

/// Start sending frames to `frames`, replacing any earlier subscriber from the same webview
/// (a reloaded page).
#[tauri::command]
pub fn visualizer_start(
    webview: tauri::Webview,
    ctl: tauri::State<'_, VisualizerCtl>,
    native: tauri::State<'_, NativeEngine>,
    frames: Channel<InvokeResponseBody>,
) -> Result<(), String> {
    let stop = Arc::new(AtomicBool::new(false));
    {
        let mut running = ctl.0.lock().map_err(|e| e.to_string())?;
        if let Some(old) = running.insert(webview.label().to_string(), Arc::clone(&stop)) {
            old.store(true, Ordering::Relaxed);
        }
    }
    let native = native.inner().clone();
    std::thread::Builder::new()
        .name("visualizer".into())
        .spawn(move || run(&native, &frames, &stop))
        .map_err(|e| format!("could not start the visualizer thread: {e}"))?;
    Ok(())
}

#[tauri::command]
pub fn visualizer_stop(
    webview: tauri::Webview,
    ctl: tauri::State<'_, VisualizerCtl>,
) -> Result<(), String> {
    if let Some(old) = ctl
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .remove(webview.label())
    {
        old.store(true, Ordering::Relaxed);
    }
    Ok(())
}

/// Open or close the visualizer window to match the setting. Built off the calling thread:
/// building a window from a synchronous command, which runs on the main thread, deadlocks on
/// Windows.
pub fn apply(app: &tauri::AppHandle, visualizer: &settings::Visualizer) {
    let existing = app.get_webview_window(WINDOW);
    match (visualizer.window, existing) {
        (true, Some(_)) | (false, None) => {}
        (true, None) => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = open_window(&app) {
                    eprintln!("[visualizer] could not open its window: {e}");
                    // The switch goes back off rather than claiming a window that isn't there.
                    let _ = settings::update(&app, |s| s.visualizer.window = false);
                }
            });
        }
        (false, Some(window)) => {
            if let Err(e) = window.destroy() {
                eprintln!("[visualizer] could not close its window: {e}");
            }
        }
    }
}

fn open_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    if app.get_webview_window(WINDOW).is_some() {
        return Ok(());
    }
    let window = WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App(WINDOW_PAGE.into()))
        .title("Jarlid Visualizer")
        .inner_size(960.0, 540.0)
        .min_inner_size(320.0, 180.0)
        // Black from the first frame, not a white flash before the page paints.
        .background_color(tauri::window::Color(0, 0, 0, 255))
        .build()?;
    let handle = app.clone();
    window.on_window_event(move |event| {
        // Closed by the listener: the setting follows. Quitting the app destroys the window
        // without asking, so one left open comes back at the next launch.
        if let tauri::WindowEvent::CloseRequested { .. } = event {
            if let Err(e) = settings::update(&handle, |s| s.visualizer.window = false) {
                eprintln!("[visualizer] could not save the window being closed: {e}");
            }
        }
    });
    Ok(())
}

fn run(native: &NativeEngine, frames: &Channel<InvokeResponseBody>, stop: &AtomicBool) {
    let mut analyser = Analyser::new();
    let mut next = Instant::now();
    let mut last_written = None;
    let mut idle_sent = false;
    while !stop.load(Ordering::Relaxed) {
        let scope = native.try_engine().and_then(|e| e.scope());
        let fresh = scope.as_ref().and_then(|s| {
            let written = analyser.read(s);
            (last_written != Some(written)).then(|| {
                last_written = Some(written);
                s.sample_rate()
            })
        });
        let out = analyser.frame(fresh, FRAME.as_secs_f32());
        // Once everything has fallen to nothing, one frame says so and then the page is left
        // alone until there is something to show.
        let idle = out.iter().all(|v| v.abs() < 1e-3);
        if !(idle && idle_sent) {
            let bytes = out.iter().flat_map(|v| v.to_le_bytes()).collect();
            if frames.send(InvokeResponseBody::Raw(bytes)).is_err() {
                // The page that asked has gone (reloaded or closed).
                break;
            }
        }
        idle_sent = idle;

        next += FRAME;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            // Fell behind (the machine was asleep, say): carry on from now, not in a burst.
            next = now;
        }
    }
}

struct Analyser {
    fft: Arc<dyn rustfft::Fft<f32>>,
    window: Vec<f32>,
    samples: Vec<f32>,
    buf: Vec<Complex32>,
    scratch: Vec<Complex32>,
    power: Vec<f32>,
    /// Per band, per bin: how much of the bin falls in the band. Built for one sample rate.
    weights: Vec<Vec<(usize, f32)>>,
    weights_rate: u32,
    peak_db: f32,
    bands: [f32; BANDS],
    level: f32,
    low: f32,
    last_low_db: f32,
    pulse: f32,
    wave: [f32; WAVE],
    wave_peak: f32,
}

impl Analyser {
    fn new() -> Self {
        let fft = FftPlanner::<f32>::new().plan_fft_forward(FFT_LEN);
        let scratch = vec![Complex32::default(); fft.get_inplace_scratch_len()];
        // Hann, normalised so a full-scale sine reads 1.0 in its bin.
        let sum: f32 = (0..FFT_LEN).map(hann).sum();
        let window = (0..FFT_LEN).map(|i| hann(i) * 2.0 / sum).collect();
        Self {
            fft,
            window,
            samples: vec![0.0; FFT_LEN],
            buf: vec![Complex32::default(); FFT_LEN],
            scratch,
            power: vec![0.0; FFT_LEN / 2],
            weights: Vec::new(),
            weights_rate: 0,
            peak_db: PEAK_FLOOR_DB,
            bands: [0.0; BANDS],
            level: 0.0,
            low: 0.0,
            last_low_db: PEAK_FLOOR_DB - RANGE_DB,
            pulse: 0.0,
            wave: [0.0; WAVE],
            wave_peak: 0.0,
        }
    }

    fn read(&mut self, scope: &audio::Scope) -> u64 {
        scope.read_heard(&mut self.samples)
    }

    /// One frame of output. `fresh` is the sample rate when new audio was read since the last
    /// frame, `None` when nothing is playing, in which case everything falls away.
    fn frame(&mut self, fresh: Option<u32>, dt: f32) -> [f32; FRAME_LEN] {
        let target = match fresh {
            Some(rate) => {
                self.trace(dt);
                self.measure(rate, dt)
            }
            None => {
                self.last_low_db = PEAK_FLOOR_DB - RANGE_DB;
                self.wave.iter_mut().for_each(|w| *w *= 0.8);
                ([0.0; BANDS], 0.0, 0.0, 0.0)
            }
        };
        let (bands, level, low, onset) = target;
        for (shown, want) in self.bands.iter_mut().zip(bands) {
            *shown = ease(*shown, want);
        }
        self.level = ease(self.level, level);
        self.low = ease(self.low, low);
        self.pulse = (self.pulse * (-dt / 0.18).exp()).max(onset);

        let mut out = [0.0; FRAME_LEN];
        out[..BANDS].copy_from_slice(&self.bands);
        out[BANDS] = self.level;
        out[BANDS + 1] = self.low;
        out[BANDS + 2] = self.pulse;
        out[BANDS + 3..].copy_from_slice(&self.wave);
        out
    }

    /// The newest stretch of the waveform, smoothed a little by averaging, and scaled to the
    /// recent peak.
    fn trace(&mut self, dt: f32) {
        let tail = &self.samples[FFT_LEN - WAVE * WAVE_STEP..];
        let mut loudest = 0.0f32;
        for (w, chunk) in self.wave.iter_mut().zip(tail.as_chunks::<WAVE_STEP>().0) {
            *w = chunk.iter().sum::<f32>() / WAVE_STEP as f32;
            loudest = loudest.max(w.abs());
        }
        // Silence stays flat rather than having its noise blown up to full height.
        self.wave_peak = (self.wave_peak * WAVE_PEAK_FALL_PER_S.powf(dt))
            .max(loudest)
            .max(0.02);
        let scale = 1.0 / self.wave_peak;
        self.wave.iter_mut().for_each(|w| *w *= scale);
    }

    fn measure(&mut self, rate: u32, dt: f32) -> ([f32; BANDS], f32, f32, f32) {
        if rate != self.weights_rate {
            self.weights = band_weights(rate);
            self.weights_rate = rate;
        }
        for ((slot, s), w) in self.buf.iter_mut().zip(&self.samples).zip(&self.window) {
            *slot = Complex32::new(s * w, 0.0);
        }
        self.fft
            .process_with_scratch(&mut self.buf, &mut self.scratch);
        for (p, c) in self.power.iter_mut().zip(&self.buf) {
            *p = c.norm_sqr();
        }

        let mut db = [0.0f32; BANDS];
        for (d, weights) in db.iter_mut().zip(&self.weights) {
            let p: f32 = weights.iter().map(|&(k, w)| self.power[k] * w).sum();
            *d = to_db(p);
        }
        let mean_square = self.samples.iter().map(|s| s * s).sum::<f32>() / FFT_LEN as f32;
        let level_db = to_db(mean_square);
        let low_db = to_db(
            self.weights[..LOW_BANDS]
                .iter()
                .flat_map(|w| w.iter().map(|&(k, w)| self.power[k] * w))
                .sum(),
        );

        let loudest = db.iter().copied().fold(f32::MIN, f32::max);
        self.peak_db = (self.peak_db - PEAK_FALL_DB_PER_S * dt)
            .max(loudest)
            .max(PEAK_FLOOR_DB);
        let scale = |d: f32| ((d - (self.peak_db - RANGE_DB)) / RANGE_DB).clamp(0.0, 1.0);

        let mut bands = [0.0; BANDS];
        for (b, d) in bands.iter_mut().zip(db) {
            *b = scale(d);
        }
        // The overall level sits below the loudest band by roughly the spread of the spectrum;
        // 12 dB of headroom puts a full mix near the top without pinning it there.
        let level = scale(level_db + 12.0);
        let low = scale(low_db);
        // A rise of 6 dB or more in the low end within one frame is a hit.
        let onset = ((low_db - self.last_low_db - 3.0) / 6.0).clamp(0.0, 1.0) * low;
        self.last_low_db = low_db;
        (bands, level, low, onset)
    }
}

fn hann(i: usize) -> f32 {
    let x = i as f32 / FFT_LEN as f32;
    0.5 - 0.5 * (std::f32::consts::TAU * x).cos()
}

fn to_db(power: f32) -> f32 {
    10.0 * power.max(1e-15).log10()
}

/// Up quickly, down gently: hits read as hits and the picture doesn't flicker.
fn ease(shown: f32, want: f32) -> f32 {
    let k = if want > shown { 0.55 } else { 0.12 };
    shown + (want - shown) * k
}

/// For each band, which FFT bins it covers and by how much. A bin covers `[k − ½, k + ½]` bin
/// widths; a band narrower than a bin (the lowest ones are) takes its share of that bin, so
/// band power stays proportional to band width and pink noise reads level.
fn band_weights(rate: u32) -> Vec<Vec<(usize, f32)>> {
    let bin_hz = rate as f32 / FFT_LEN as f32;
    let high = HIGH_HZ.min(rate as f32 / 2.0 - bin_hz);
    let ratio = (high / LOW_HZ).powf(1.0 / BANDS as f32);
    (0..BANDS)
        .map(|b| {
            let lo = LOW_HZ * ratio.powi(b as i32) / bin_hz;
            let hi = LOW_HZ * ratio.powi(b as i32 + 1) / bin_hz;
            let first = (lo + 0.5).floor().max(1.0) as usize;
            let last = ((hi + 0.5).ceil() as usize).min(FFT_LEN / 2 - 1);
            (first..=last)
                .filter_map(|k| {
                    let overlap = (hi.min(k as f32 + 0.5) - lo.max(k as f32 - 0.5)).max(0.0);
                    (overlap > 0.0).then_some((k, overlap))
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, rate: u32, amp: f32) -> Vec<f32> {
        (0..FFT_LEN)
            .map(|i| amp * (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin())
            .collect()
    }

    fn band_of(hz: f32, rate: u32) -> usize {
        let bin = hz * FFT_LEN as f32 / rate as f32;
        band_weights(rate)
            .iter()
            .position(|w| w.iter().any(|&(k, _)| (k as f32 - bin).abs() < 0.5))
            .expect("in range")
    }

    #[test]
    fn every_bin_in_range_is_counted_once() {
        let weights = band_weights(48_000);
        let bin_hz = 48_000.0 / FFT_LEN as f32;
        let first = (LOW_HZ / bin_hz + 0.5).floor() as usize + 1;
        let last = (HIGH_HZ / bin_hz).floor() as usize - 1;
        for k in first..last {
            let total: f32 = weights
                .iter()
                .flat_map(|w| w.iter())
                .filter(|&&(bin, _)| bin == k)
                .map(|&(_, w)| w)
                .sum();
            assert!((total - 1.0).abs() < 1e-3, "bin {k} counted {total}");
        }
    }

    #[test]
    fn a_tone_lights_its_own_band() {
        let rate = 48_000;
        let mut a = Analyser::new();
        a.samples = sine(1000.0, rate, 0.5);
        let (bands, ..) = a.measure(rate, FRAME.as_secs_f32());
        let peak = (0..BANDS)
            .max_by(|&x, &y| bands[x].total_cmp(&bands[y]))
            .unwrap();
        assert_eq!(peak, band_of(1000.0, rate));
        assert!(bands[peak] > 0.95, "{bands:?}");
        assert!(bands[band_of(100.0, rate)] < 0.2, "{bands:?}");
    }

    #[test]
    fn silence_draws_nothing() {
        let mut a = Analyser::new();
        a.samples = vec![0.0; FFT_LEN];
        let (bands, level, low, onset) = a.measure(48_000, FRAME.as_secs_f32());
        assert!(bands.iter().all(|b| *b == 0.0));
        assert_eq!((level, low, onset), (0.0, 0.0, 0.0));
    }

    #[test]
    fn a_kick_after_quiet_is_a_pulse() {
        let rate = 48_000;
        let mut a = Analyser::new();
        a.samples = sine(60.0, rate, 0.01);
        a.measure(rate, FRAME.as_secs_f32());
        a.samples = sine(60.0, rate, 0.8);
        let (.., onset) = a.measure(rate, FRAME.as_secs_f32());
        assert!(onset > 0.9, "onset {onset}");
    }

    #[test]
    fn the_wave_is_scaled_to_its_peak() {
        let rate = 48_000;
        let mut a = Analyser::new();
        a.samples = sine(375.0, rate, 0.1);
        a.trace(FRAME.as_secs_f32());
        let top = a.wave.iter().fold(0.0f32, |m, w| m.max(w.abs()));
        assert!((top - 1.0).abs() < 1e-3, "{top}");
        // Silence is flat, however long it lasts.
        a.samples = vec![0.0; FFT_LEN];
        for _ in 0..600 {
            a.trace(FRAME.as_secs_f32());
        }
        assert!(a.wave.iter().all(|w| *w == 0.0));
    }

    #[test]
    fn nothing_playing_falls_away() {
        let mut a = Analyser::new();
        a.bands = [1.0; BANDS];
        for _ in 0..120 {
            a.frame(None, FRAME.as_secs_f32());
        }
        assert!(a.bands.iter().all(|b| *b < 1e-3));
    }
}
