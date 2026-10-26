//! Score drafts — lyrics written from the heard words alone — against the corpus's real lyrics.
//!
//! Every corpus track has LRCLIB lyrics, which the drafter never sees, so each one is a test of
//! what the reviewer would be handed for a song with none:
//!
//! - **WER**: word error rate of the draft's words against `plain.txt`. What the reviewer has to
//!   retype.
//! - **Lines**: draft line starts against `synced.lrc`'s. Recall is the share of real lines with
//!   a draft line starting within the tolerance; precision the share of draft lines that start
//!   within it of a real one. What the reviewer has to re-split or retime.
//! - **Doubt**: of the words marked doubtful, the share that really were wrong (precision), and
//!   of the wrong words, the share marked (recall). Whether the marks point at the mistakes.
//!
//! Writes each draft beside the model output as `<track>.draft.lrc`, with doubtful words in
//! `[[brackets]]`, for eyeballing.
//!
//! Drafting parameters can be overridden as `DRAFT_<FIELD>=<value>` (`DRAFT_PAUSE=0.6`).
//!
//! Run: cargo run --release -p autosync --example draft -- <corpus> <results> <config>

mod heard;

use std::path::Path;

use autosync::draft::{self, Params};
use autosync::lrc;
use autosync::text::words;
use heard::{load_heard, Timing};

/// Line starts within this many seconds are taken to be the same line.
const TOLERANCE: [f64; 2] = [0.5, 1.0];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        eprintln!("usage: draft <corpus> <results> <config>");
        std::process::exit(2);
    }
    let corpus = Path::new(&args[0]);
    let results = Path::new(&args[1]).join(&args[2]);
    let params = params_from_env();
    eprintln!("{params:?}");

    let mut entries: Vec<_> = std::fs::read_dir(corpus)
        .expect("corpus dir")
        .flatten()
        .map(|e| e.path())
        .collect();
    entries.sort();

    let quiet = std::env::var("DRAFT_QUIET").is_ok();
    if !quiet {
        header();
    }
    let mut total = Totals::default();
    for dir in entries {
        let track = dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let Some(heard) = load_heard(&results, &track, Timing::Dtw) else {
            continue;
        };
        let (Ok(plain), Ok(synced)) = (
            std::fs::read_to_string(dir.join("plain.txt")),
            std::fs::read_to_string(dir.join("synced.lrc")),
        ) else {
            continue;
        };
        let d = draft::draft_with(&heard, &params).unwrap_or_else(|why| {
            if !quiet {
                println!("{track}: no draft, {why:?}");
            }
            Vec::new()
        });
        let _ = std::fs::write(
            results.join(format!("{track}.draft.lrc")),
            annotated(&d, &params),
        );

        let t = score(&d, &plain, &synced, &params);
        if !quiet {
            row(&track, &t);
        }
        total.add(&t);
    }
    if quiet {
        header();
    } else {
        println!();
    }
    let t = total;
    row(
        &format!("ALL ({} tracks, {} empty drafts)", t.tracks, t.empty),
        &t,
    );
    println!(
        "draft words {} of which wrong {} ({:.0}%), marked doubtful {} ({:.0}%)",
        t.draft_words,
        t.wrong,
        pct(t.wrong, t.draft_words),
        t.doubted,
        pct(t.doubted, t.draft_words)
    );
}

fn header() {
    println!(
        "{:<44} {:>5} {:>5} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6}",
        "track", "lines", "draft", "WER", "rec.5", "prec.5", "rec1", "rec1o", "doubtP", "doubtR"
    );
}

/// `rec1o` is `rec1` with the track's own constant offset removed: some LRCLIB copies are synced
/// to another master, which no drafter could match.
fn row(label: &str, t: &Totals) {
    println!(
        "{:<44} {:>5} {:>5} {:>5.0}% {:>5.0}% {:>5.0}% {:>5.0}% {:>5.0}% {:>5.0}% {:>5.0}%",
        label.chars().take(44).collect::<String>(),
        t.truth_lines,
        t.draft_lines,
        pct(t.errors, t.ref_words),
        pct(t.recalled[0], t.truth_lines),
        pct(t.precise[0], t.draft_lines),
        pct(t.recalled[1], t.truth_lines),
        pct(t.recalled_centred, t.truth_lines),
        pct(t.doubted_wrong, t.doubted),
        pct(t.doubted_wrong, t.wrong),
    );
}

#[derive(Default)]
struct Totals {
    tracks: usize,
    empty: usize,
    /// Words in the real lyrics, and edits needed to turn the draft's words into them.
    ref_words: usize,
    errors: usize,
    truth_lines: usize,
    draft_lines: usize,
    recalled: [usize; 2],
    precise: [usize; 2],
    /// Recalled within 1 s once the track's median offset is removed.
    recalled_centred: usize,
    /// Heard words in the draft; of those, wrong ones, doubtful ones, and both.
    draft_words: usize,
    wrong: usize,
    doubted: usize,
    doubted_wrong: usize,
}

impl Totals {
    fn add(&mut self, t: &Totals) {
        self.tracks += 1;
        self.empty += usize::from(t.draft_lines == 0);
        self.ref_words += t.ref_words;
        self.errors += t.errors;
        self.truth_lines += t.truth_lines;
        self.draft_lines += t.draft_lines;
        for k in 0..2 {
            self.recalled[k] += t.recalled[k];
            self.precise[k] += t.precise[k];
        }
        self.recalled_centred += t.recalled_centred;
        self.draft_words += t.draft_words;
        self.wrong += t.wrong;
        self.doubted += t.doubted;
        self.doubted_wrong += t.doubted_wrong;
    }
}

fn score(d: &[draft::Line], plain: &str, synced: &str, params: &Params) -> Totals {
    let mut t = Totals::default();

    // Words: each heard word may normalise to several ("a-holding") or none ("—"); errors are
    // charged back to the heard word they came from.
    let reference = words(plain);
    let mut ours: Vec<String> = Vec::new();
    let mut owner: Vec<usize> = Vec::new();
    let drafted: Vec<&draft::Word> = d.iter().flat_map(|l| &l.words).collect();
    for (k, w) in drafted.iter().enumerate() {
        for n in words(&w.text) {
            ours.push(n);
            owner.push(k);
        }
    }
    let (errors, wrong_tokens) = edit_script(&ours, &reference);
    t.ref_words = reference.len();
    t.errors = errors;
    let mut wrong = vec![false; drafted.len()];
    for i in wrong_tokens {
        wrong[owner[i]] = true;
    }
    t.draft_words = drafted.len();
    for (k, w) in drafted.iter().enumerate() {
        let doubtful = w.p < params.doubt;
        t.wrong += usize::from(wrong[k]);
        t.doubted += usize::from(doubtful);
        t.doubted_wrong += usize::from(doubtful && wrong[k]);
    }

    // Lines: only lines with words, on both sides.
    let truth: Vec<f64> = lrc::parse(synced)
        .into_iter()
        .filter(|l| !words(&l.text).is_empty())
        .map(|l| l.t)
        .collect();
    let starts: Vec<f64> = d
        .iter()
        .filter(|l| !l.words.is_empty())
        .map(|l| l.t)
        .collect();
    t.truth_lines = truth.len();
    t.draft_lines = starts.len();
    let near = |t: f64, set: &[f64], tol: f64| set.iter().any(|s| (s - t).abs() <= tol);
    for (k, tol) in TOLERANCE.iter().enumerate() {
        t.recalled[k] = truth.iter().filter(|&&x| near(x, &starts, *tol)).count();
        t.precise[k] = starts.iter().filter(|&&x| near(x, &truth, *tol)).count();
    }
    // The track's offset: the median of each real line's distance to its nearest draft line.
    let mut nearest: Vec<f64> = truth
        .iter()
        .filter_map(|&x| {
            starts
                .iter()
                .map(|s| s - x)
                .min_by(|a, b| a.abs().total_cmp(&b.abs()))
        })
        .collect();
    nearest.sort_by(f64::total_cmp);
    let offset = nearest.get(nearest.len() / 2).copied().unwrap_or_default();
    t.recalled_centred = truth
        .iter()
        .filter(|&&x| near(x + offset, &starts, 1.0))
        .count();
    t
}

/// Word-level edit distance from `ours` to `reference`, and which of `ours` were substituted or
/// inserted (that is, wrong).
fn edit_script(ours: &[String], reference: &[String]) -> (usize, Vec<usize>) {
    let (n, m) = (ours.len(), reference.len());
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in dp.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in dp[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let sub = dp[i - 1][j - 1] + usize::from(ours[i - 1] != reference[j - 1]);
            dp[i][j] = sub.min(dp[i - 1][j] + 1).min(dp[i][j - 1] + 1);
        }
    }
    let mut wrong = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        if i > 0
            && j > 0
            && dp[i][j] == dp[i - 1][j - 1] + usize::from(ours[i - 1] != reference[j - 1])
        {
            if ours[i - 1] != reference[j - 1] {
                wrong.push(i - 1);
            }
            i -= 1;
            j -= 1;
        } else if i > 0 && dp[i][j] == dp[i - 1][j] + 1 {
            wrong.push(i - 1);
            i -= 1;
        } else {
            j -= 1;
        }
    }
    (dp[n][m], wrong)
}

/// The draft as LRC with doubtful words in `[[brackets]]`.
fn annotated(d: &[draft::Line], params: &Params) -> String {
    let lines: Vec<lrc::Line> = d
        .iter()
        .map(|l| {
            let doubt = l.doubtful(params);
            let text = l.text();
            let marked = text
                .split(' ')
                .enumerate()
                .map(|(i, w)| {
                    if doubt.contains(&i) {
                        format!("[[{w}]]")
                    } else {
                        w.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            lrc::Line {
                t: l.t,
                text: marked,
            }
        })
        .collect();
    lrc::format(&lines)
}

fn pct(a: usize, b: usize) -> f64 {
    a as f64 * 100.0 / b.max(1) as f64
}

fn params_from_env() -> Params {
    let mut p = Params::default();
    let get = |name: &str| std::env::var(format!("DRAFT_{name}")).ok();
    if let Some(v) = get("PAUSE").and_then(|v| v.parse().ok()) {
        p.pause = v;
    }
    if let Some(v) = get("PUNCT_PAUSE").and_then(|v| v.parse().ok()) {
        p.punct_pause = v;
    }
    if let Some(v) = get("MAX_WORDS").and_then(|v| v.parse().ok()) {
        p.max_words = v;
    }
    if let Some(v) = get("MAX_SECONDS").and_then(|v| v.parse().ok()) {
        p.max_seconds = v;
    }
    if let Some(v) = get("MIN_WORDS").and_then(|v| v.parse().ok()) {
        p.min_words = v;
    }
    if let Some(v) = get("GAP_LINE").and_then(|v| v.parse().ok()) {
        p.gap_line = v;
    }
    if let Some(v) = get("LEAD").and_then(|v| v.parse().ok()) {
        p.lead = v;
    }
    if let Some(v) = get("MIN_CONFIDENCE").and_then(|v| v.parse().ok()) {
        p.min_confidence = v;
    }
    if let Some(v) = get("DOUBT").and_then(|v| v.parse().ok()) {
        p.doubt = v;
    }
    p
}
