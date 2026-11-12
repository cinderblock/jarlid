//! Word starts for line-timed lyrics, from case files: run and score [`autosync::words`].
//!
//! A case is JSON: `{"lines":[{"text","start","end"}], "heard":[[text,start,end,p],…],
//! "truth":[[start,…]|null,…]}`. `heard` is what the speech engine heard, or another
//! transcription's timed words. `truth`, when
//! present, is known word starts by line, and the case is scored against it: the error of every
//! word but each line's first (which is the line's start, so always right), next to what
//! spreading the words by syllables alone would have got.
//!
//! Writes `<case>.out.json` = `[{"starts":[…],"matched":n},…]` beside each case.
//!
//! Run: cargo run --release -p autosync --example words -- <case.json>…
//! `WORDS_<FIELD>=<value>` overrides a parameter (`WORDS_SLACK=0.4`).

use autosync::words::{time_words, Line, Params};
use autosync::Heard;
use serde_json::{json, Value};

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("usage: words <case.json>…");
        std::process::exit(2);
    }
    let mut all = (Vec::new(), Vec::new());
    println!(
        "{:<40} {:>5} {:>6} | {:>6} {:>6} {:>5} {:>5} | {:>6} {:>6}",
        "case", "words", "match", "med", "p90", "≤.1", "≤.25", "b.med", "b.p90"
    );
    for f in &files {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(f).expect("read case"))
            .expect("case JSON");
        let texts: Vec<String> = v["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["text"].as_str().unwrap_or("").to_string())
            .collect();
        let lines: Vec<Line> = v["lines"]
            .as_array()
            .unwrap()
            .iter()
            .zip(&texts)
            .map(|(l, text)| Line {
                text,
                start: l["start"].as_f64().unwrap(),
                end: l["end"].as_f64().unwrap(),
            })
            .collect();
        let heard: Vec<Heard> = v["heard"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|w| Heard {
                        text: w[0].as_str().unwrap_or("").into(),
                        start: w[1].as_f64().unwrap(),
                        end: w[2].as_f64().unwrap(),
                        p: w[3].as_f64().unwrap_or(1.0) as f32,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut p = Params::default();
        overrides(&mut p);
        let out = time_words(&lines, &heard, &p);
        let base = time_words(&lines, &[], &p);
        let _ = std::fs::write(
            f.trim_end_matches(".json").to_string() + ".out.json",
            serde_json::to_string(
                &out.iter()
                    .map(|l| json!({"starts": l.starts, "matched": l.matched}))
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
        );
        let name = std::path::Path::new(f)
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let words: usize = out.iter().map(|l| l.starts.len().saturating_sub(1)).sum();
        let matched: usize = out.iter().map(|l| l.matched).sum();
        let Some(truth) = v["truth"].as_array() else {
            println!("{name:<40} {words:>5} {matched:>6}");
            continue;
        };
        let (mut e, mut b) = (Vec::new(), Vec::new());
        for ((t, o), bl) in truth.iter().zip(&out).zip(&base) {
            let Some(t) = t.as_array() else { continue };
            if t.len() != o.starts.len() {
                continue;
            }
            for k in 1..t.len() {
                let tk = t[k].as_f64().unwrap();
                e.push((o.starts[k] - tk).abs());
                b.push((bl.starts[k] - tk).abs());
            }
        }
        let (s, bs) = (stats(&mut e), stats(&mut b));
        println!(
            "{:<40} {:>5} {:>6} | {:>6.2} {:>6.2} {:>4.0}% {:>4.0}% | {:>6.2} {:>6.2}",
            &name[..name.len().min(40)],
            words,
            matched,
            s.0,
            s.1,
            s.2 * 100.0,
            s.3 * 100.0,
            bs.0,
            bs.1
        );
        all.0.extend(e);
        all.1.extend(b);
    }
    if !all.0.is_empty() {
        let (s, bs) = (stats(&mut all.0), stats(&mut all.1));
        println!(
            "{:<40} {:>5} {:>6} | {:>6.2} {:>6.2} {:>4.0}% {:>4.0}% | {:>6.2} {:>6.2}",
            "ALL",
            all.0.len(),
            "",
            s.0,
            s.1,
            s.2 * 100.0,
            s.3 * 100.0,
            bs.0,
            bs.1
        );
    }
}

/// Median, p90, share within 0.1 s, share within 0.25 s.
fn stats(e: &mut [f64]) -> (f64, f64, f64, f64) {
    if e.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    e.sort_by(f64::total_cmp);
    let n = e.len() as f64;
    (
        e[e.len() / 2],
        e[(e.len() * 9 / 10).min(e.len() - 1)],
        e.iter().filter(|&&x| x <= 0.1).count() as f64 / n,
        e.iter().filter(|&&x| x <= 0.25).count() as f64 / n,
    )
}

fn overrides(p: &mut Params) {
    let get = |k: &str| {
        std::env::var(format!("WORDS_{k}"))
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
    };
    if let Some(v) = get("MATCH_SIM") {
        p.match_sim = v;
    }
    if let Some(v) = get("SLACK") {
        p.slack = v;
    }
    if let Some(v) = get("LEAD") {
        p.lead = v;
    }
    if let Some(v) = get("SYLLABLE_SECONDS") {
        p.syllable_seconds = v;
    }
}
