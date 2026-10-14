//! Turning lyric lines and transcripts into comparable words.
//!
//! Both sides go through [`words`], so the comparison never sees case, punctuation or the
//! difference between `don't`, `don’t` and `dont`. Whisper writes numbers as digits about as
//! often as lyrics spell them out; that mismatch is left to the fuzzy match in [`similarity`]
//! rather than guessed at here.

/// The words of `s`, lowercased, with punctuation and apostrophes removed.
pub fn words(s: &str) -> Vec<String> {
    s.split(|c: char| c.is_whitespace() || matches!(c, '-' | '–' | '—' | '/' | '…'))
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// How alike two normalised words are, 0–1: one minus their edit distance over the longer length.
///
/// Sung words come back slightly wrong far more often than completely wrong — "gonna" for
/// "going", "tryna" for "trying to" — so near misses have to count for something.
pub fn similarity(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let longest = a.len().max(b.len());
    if longest == 0 {
        return 1.0;
    }
    1.0 - levenshtein(&a, &b) as f64 / longest as f64
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = if ca == cb {
                diagonal
            } else {
                1 + diagonal.min(above).min(row[j])
            };
            diagonal = above;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_ignore_case_punctuation_and_apostrophes() {
        assert_eq!(
            words("Don’t stop — me now, (DON'T stop me!)"),
            ["dont", "stop", "me", "now", "dont", "stop", "me"]
        );
    }

    #[test]
    fn words_keep_non_latin_letters() {
        assert_eq!(
            words("Ich liebe dich, señorita"),
            ["ich", "liebe", "dich", "señorita"]
        );
    }

    #[test]
    fn similarity_scores_near_misses() {
        assert_eq!(similarity("love", "love"), 1.0);
        assert!(similarity("gonna", "gona") > 0.7);
        assert!(similarity("love", "table") < 0.3);
    }
}
