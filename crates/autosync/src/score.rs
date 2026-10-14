//! Scoring generated line times against a known-good synced copy of the same lyrics.
//!
//! The plain and synced copies of an LRCLIB record are usually the same lines in the same order,
//! but not always exactly: the synced one has blank timestamp lines for instrumental gaps, and
//! either may have a line the other lacks. So lines are paired by aligning their texts first, and
//! only paired lines are scored.

use crate::lrc::Line;
use crate::text::{similarity, words};

/// Lines whose texts are at least this alike are taken to be the same line.
const SAME_LINE: f64 = 0.8;

/// How far off a set of generated times is.
#[derive(Debug, Clone, Default)]
pub struct Score {
    /// Non-blank lines in the plain text.
    pub lines: usize,
    /// Of those, how many could be paired with a synced line.
    pub paired: usize,
    /// Of the paired lines, how many were given a time at all.
    pub timed: usize,
    /// `generated − truth`, seconds, one per timed paired line.
    pub errors: Vec<f64>,
}

impl Score {
    /// Median absolute error, seconds.
    pub fn median_abs(&self) -> Option<f64> {
        percentile(self.errors.iter().map(|e| e.abs()).collect(), 0.5)
    }

    /// 90th-percentile absolute error, seconds.
    pub fn p90_abs(&self) -> Option<f64> {
        percentile(self.errors.iter().map(|e| e.abs()).collect(), 0.9)
    }

    /// Median signed error. Negative means early.
    pub fn median_signed(&self) -> Option<f64> {
        percentile(self.errors.clone(), 0.5)
    }

    /// Share of *paired* lines timed to within `seconds`. Untimed lines count as misses.
    pub fn within(&self, seconds: f64) -> f64 {
        let hits = self.errors.iter().filter(|e| e.abs() <= seconds).count();
        hits as f64 / self.paired.max(1) as f64
    }
}

/// Score `generated` (one optional time per line of `plain`) against `truth`.
pub fn score(plain: &[&str], generated: &[Option<f64>], truth: &[Line]) -> Score {
    let truth: Vec<&Line> = truth
        .iter()
        .filter(|l| !words(&l.text).is_empty())
        .collect();
    let ours: Vec<usize> = (0..plain.len())
        .filter(|&i| !words(plain[i]).is_empty())
        .collect();

    let a: Vec<String> = ours.iter().map(|&i| words(plain[i]).join(" ")).collect();
    let b: Vec<String> = truth.iter().map(|l| words(&l.text).join(" ")).collect();

    let mut out = Score {
        lines: ours.len(),
        ..Default::default()
    };
    for (x, y) in pair_lines(&a, &b) {
        out.paired += 1;
        if let Some(t) = generated[ours[x]] {
            out.timed += 1;
            out.errors.push(t - truth[y].t);
        }
    }
    out
}

/// Pairs `(index in a, index in b)` of matching lines, in order (longest common subsequence
/// under fuzzy equality).
fn pair_lines(a: &[String], b: &[String]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let width = m + 1;
    let mut len = vec![0u32; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            len[i * width + j] = if similarity(&a[i], &b[j]) >= SAME_LINE {
                1 + len[(i + 1) * width + j + 1]
            } else {
                len[(i + 1) * width + j].max(len[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < n && j < m {
        if similarity(&a[i], &b[j]) >= SAME_LINE {
            out.push((i, j));
            i += 1;
            j += 1;
        } else if len[(i + 1) * width + j] >= len[i * width + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

fn percentile(mut values: Vec<f64>, q: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let at = ((values.len() - 1) as f64 * q).round() as usize;
    Some(values[at])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(t: f64, text: &str) -> Line {
        Line {
            t,
            text: text.into(),
        }
    }

    #[test]
    fn pairs_lines_across_blank_gaps_and_scores_them() {
        let plain = ["first line", "", "second line", "extra line not synced"];
        let truth = [
            line(1.0, "First line"),
            line(4.0, ""),
            line(6.0, "second line!"),
        ];
        let s = score(&plain, &[Some(1.5), None, Some(5.0), Some(9.0)], &truth);
        assert_eq!((s.lines, s.paired, s.timed), (3, 2, 2));
        assert_eq!(s.errors, [0.5, -1.0]);
        assert_eq!(s.within(0.5), 0.5);
    }
}
