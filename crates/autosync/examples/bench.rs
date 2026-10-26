//! Score speech-model output against the corpus's known-good synced lyrics.
//!
//! For each track that has output under `<results>/<config>/`, align the track's *plain* lyrics
//! to the heard words and compare the line times with `synced.lrc`, which the aligner never sees.
//! Writes the generated LRC beside the model output for eyeballing, and prints one row per track
//! and a summary per config.
//!
//! Model output is read by [`heard`]. Whisper runs are scored twice, with DTW token stamps and
//! with token offsets, as `<config>` and `<config>+offsets`.
//!
//! Run: cargo run --release -p autosync --example bench -- <corpus> <results> <config>…

mod heard;

use std::path::Path;

use autosync::{align, lrc, score};
use heard::{load_heard, Timing};
use serde_json::Value;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("usage: bench <corpus> <results> <config>…");
        std::process::exit(2);
    }
    let corpus = Path::new(&args[0]);
    let results = Path::new(&args[1]);

    let params = params_from_env();
    eprintln!("{params:?}");
    let mut summaries = Vec::new();
    for config in &args[2..] {
        let variants: &[(&str, Timing)] = if config.starts_with("whisper") {
            &[("", Timing::Dtw), ("+offsets", Timing::Offsets)]
        } else {
            &[("", Timing::Dtw)]
        };
        for (suffix, timing) in variants {
            let name = format!("{config}{suffix}");
            println!("\n== {name}");
            println!(
                "{:<48} {:>5} {:>6} {:>6} {:>7} {:>7} {:>6} {:>6}",
                "track", "lines", "cover", "med", "signed", "p90", "<0.3s", "<1s"
            );
            // Pooled four ways: every track as-is; with each track's own median signed error
            // removed (LRCLIB copies are sometimes synced to another master, which shows up as
            // a constant offset that is not the aligner's doing); and both again keeping only
            // tracks a coverage gate would accept.
            let mut pools: Vec<(String, usize, score::Score)> = [
                "",
                " (offset removed)",
                &format!(" (cover>={GATE:.0}%)", GATE = GATE * 100.0),
                &format!(" (cover>={GATE:.0}%, offset removed)", GATE = GATE * 100.0),
            ]
            .iter()
            .map(|label| (format!("{name}{label}"), 0, score::Score::default()))
            .collect();
            let mut entries: Vec<_> = std::fs::read_dir(corpus)
                .expect("corpus dir")
                .flatten()
                .map(|e| e.path())
                .collect();
            entries.sort();
            for dir in entries {
                let track = dir
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let Some(heard) = load_heard(&results.join(config), &track, *timing) else {
                    continue;
                };
                let (Ok(plain), Ok(synced)) = (
                    std::fs::read_to_string(dir.join("plain.txt")),
                    std::fs::read_to_string(dir.join("synced.lrc")),
                ) else {
                    continue;
                };
                let lines: Vec<&str> = plain.lines().collect();
                let aligned = align::align_with(&lines, &heard, &params);
                // BENCH_DUMP=<track> prints what was heard, to see why a line landed where it did.
                if std::env::var("BENCH_DUMP").is_ok_and(|want| want == track) {
                    for h in &heard {
                        println!(
                            "  heard {:8.2} {:8.2} {:.2} {}",
                            h.start, h.end, h.p, h.text
                        );
                    }
                    for (l, text) in aligned.lines.iter().zip(&lines) {
                        println!(
                            "  line  {:>8} {}/{} {} {}",
                            secs(l.t),
                            l.matched,
                            l.words,
                            if l.interpolated { "interp" } else { "      " },
                            text
                        );
                    }
                }
                // Scored as the lyrics pane shows them: every line timed, including those before
                // the first anchor and after the last.
                let duration = duration_of(&dir);
                let times = aligned.placed(duration);
                let s = score::score(&lines, &times, &lrc::parse(&synced));
                let _ = std::fs::write(
                    results.join(config).join(format!("{track}{suffix}.lrc")),
                    lrc::format(&aligned.timed(&lines, duration)),
                );

                println!(
                    "{:<48} {:>5} {:>5.0}% {:>6} {:>7} {:>7} {:>5.0}% {:>5.0}%",
                    track.chars().take(48).collect::<String>(),
                    s.paired,
                    aligned.coverage() * 100.0,
                    secs(s.median_abs()),
                    secs(s.median_signed()),
                    secs(s.p90_abs()),
                    s.within(0.3) * 100.0,
                    s.within(1.0) * 100.0,
                );
                let offset = s.median_signed().unwrap_or_default();
                let centred: Vec<f64> = s.errors.iter().map(|e| e - offset).collect();
                let passes = aligned.coverage() >= GATE;
                for (k, (_, tracks, pool)) in pools.iter_mut().enumerate() {
                    if k >= 2 && !passes {
                        continue;
                    }
                    pool.lines += s.lines;
                    pool.paired += s.paired;
                    pool.timed += s.timed;
                    pool.errors
                        .extend(if k % 2 == 1 { &centred } else { &s.errors });
                    *tracks += 1;
                }
            }
            summaries.extend(pools);
        }
    }

    println!("\n== summary (all lines of all tracks pooled)");
    println!(
        "{:<52} {:>6} {:>6} {:>6} {:>7} {:>7} {:>6} {:>6} {:>6} {:>6}",
        "config", "tracks", "lines", "med", "signed", "p90", "<0.3s", "<0.5s", "<1s", "<3s"
    );
    for (name, tracks, s) in summaries {
        println!(
            "{:<52} {:>6} {:>6} {:>6} {:>7} {:>7} {:>5.0}% {:>5.0}% {:>5.0}% {:>5.0}%",
            name,
            tracks,
            s.paired,
            secs(s.median_abs()),
            secs(s.median_signed()),
            secs(s.p90_abs()),
            s.within(0.3) * 100.0,
            s.within(0.5) * 100.0,
            s.within(1.0) * 100.0,
            s.within(3.0) * 100.0,
        );
    }
}

/// The aligner's defaults, with any `AUTOSYNC_<FIELD>` environment variable overriding its field
/// (`AUTOSYNC_LEAD=0.15`), so a sweep needs no rebuild.
fn params_from_env() -> align::Params {
    let mut p = align::Params::default();
    let get = |name: &str| std::env::var(format!("AUTOSYNC_{name}")).ok();
    if let Some(v) = get("MATCH_SIM").and_then(|v| v.parse().ok()) {
        p.match_sim = v;
    }
    if let Some(v) = get("GAP").and_then(|v| v.parse().ok()) {
        p.gap = v;
    }
    if let Some(v) = get("MISMATCH").and_then(|v| v.parse().ok()) {
        p.mismatch = v;
    }
    if let Some(v) = get("MIN_ANCHOR_WORDS").and_then(|v| v.parse().ok()) {
        p.min_anchor_words = v;
    }
    if let Some(v) = get("LEAD").and_then(|v| v.parse().ok()) {
        p.lead = v;
    }
    if let Some(v) = get("UNHEARD_WORD_SECONDS").and_then(|v| v.parse().ok()) {
        p.unheard_word_seconds = v;
    }
    if let Some(v) = get("CROWDING").and_then(|v| v.parse().ok()) {
        p.crowding = v;
    }
    p
}

/// Coverage below which a result would be rejected rather than shown as synced.
const GATE: f64 = 0.6;

/// The track's length from the corpus's `meta.json` (Pandora's, which is what the app has), or
/// unbounded when it isn't there.
fn duration_of(dir: &Path) -> f64 {
    std::fs::read_to_string(dir.join("meta.json"))
        .ok()
        .and_then(|json| serde_json::from_str::<Value>(&json).ok())
        .and_then(|v| v["pandoraLength"].as_f64())
        .unwrap_or(f64::MAX)
}

fn secs(v: Option<f64>) -> String {
    v.map_or("-".into(), |v| format!("{v:.2}"))
}
