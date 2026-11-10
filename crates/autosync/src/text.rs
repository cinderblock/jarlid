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

/// Whether `line` is a section label ("Chorus:", "[Verse 2: Artist]", "(Bridge)", "Chorus x2")
/// rather than a sung line. Labels are never sung, so they are never aligned or timed.
///
/// The Rust twin of `sectionLabel` in `app/src/sections.ts`: change both together.
pub fn is_section_label(line: &str) -> bool {
    const NAMES: &[&str] = &[
        "intro",
        "verse",
        "pre chorus",
        "prechorus",
        "chorus",
        "post chorus",
        "postchorus",
        "hook",
        "refrain",
        "bridge",
        "interlude",
        "breakdown",
        "break",
        "instrumental",
        "solo",
        "outro",
        "coda",
        "drop",
        "tag",
    ];
    // Names that can also be a whole sung line need brackets, a colon or a number.
    const NEEDS_MARK: &[&str] = &["hook", "breakdown", "break", "solo", "coda", "drop", "tag"];
    const ROMAN: &[&str] = &["i", "ii", "iii", "iv", "v", "vi", "vii", "viii", "ix", "x"];

    let mut s = line.trim();
    if s.is_empty() || s.chars().count() > 60 {
        return false;
    }
    let mut marked = false;
    let close = match s.chars().next() {
        Some('[') => Some(']'),
        Some('(') => Some(')'),
        Some('{') => Some('}'),
        _ => None,
    };
    if let Some(c) = close {
        if s.len() > 1 && s.ends_with(c) {
            s = s[1..s.len() - 1].trim();
            marked = true;
        }
    }
    // Who sings it, after a colon or a spaced dash inside brackets.
    let sep = marked
        .then(|| {
            [":", " - ", " – ", " — "]
                .iter()
                .filter_map(|p| s.find(p))
                .min()
        })
        .flatten();
    if let Some(i) = sep.filter(|&i| i > 0) {
        s = s[..i].trim_end();
    } else if let Some(t) = s.strip_suffix(':') {
        s = t.trim_end();
        marked = true;
    }
    s = strip_repeat(s);

    let lower = s.to_lowercase().replace(['-', '–'], " ");
    let mut words: Vec<&str> = lower.split_whitespace().collect();
    let mut numbered = false;
    if words.len() > 1 {
        let last = words[words.len() - 1];
        let digits = (1..=2).contains(&last.len()) && last.chars().all(|c| c.is_ascii_digit());
        if digits || ROMAN.contains(&last) {
            words.pop();
            numbered = true;
        }
    }
    let name = words.join(" ");
    NAMES.contains(&name.as_str()) && (marked || numbered || !NEEDS_MARK.contains(&name.as_str()))
}

/// `s` without a repeat count: "Chorus x2", "Chorus (x2)", "Chorus ×2", "Chorus 2x", "2x Chorus".
fn strip_repeat(s: &str) -> &str {
    let is_x = |c: char| matches!(c, 'x' | 'X' | '×');
    // Leading "2x ".
    let digits = s.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let rest = &s[digits..];
        if let Some(c) = rest.chars().next().filter(|&c| is_x(c)) {
            let after = &rest[c.len_utf8()..];
            if after.starts_with(char::is_whitespace) {
                return after.trim_start();
            }
        }
    }
    let mut t = s.trim_end();
    let paren = t.ends_with(')');
    if paren {
        t = t[..t.len() - 1].trim_end();
    }
    let before_digits = t.trim_end_matches(|c: char| c.is_ascii_digit());
    let rest = if before_digits.len() < t.len() {
        // "x2": the digits come after the x.
        before_digits
            .trim_end()
            .strip_suffix(is_x)
            .map(str::trim_end)
    } else {
        // "2x": the x comes after the digits.
        t.strip_suffix(is_x)
            .map(|u| u.trim_end_matches(|c: char| c.is_ascii_digit()))
            .filter(|u| u.len() + 1 < t.len())
            .map(str::trim_end)
    };
    let Some(mut rest) = rest else { return s };
    if paren {
        match rest.strip_suffix('(') {
            Some(r) => rest = r.trim_end(),
            None => return s,
        }
    }
    if rest.is_empty() || !s[rest.len()..].starts_with(|c: char| c.is_whitespace() || c == '(') {
        return s;
    }
    rest
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

    #[test]
    fn section_labels_in_their_usual_spellings() {
        for l in [
            "Chorus:",
            "Verse 1:",
            "Pre-chorus:",
            "[Chorus]",
            "(Bridge)",
            "[Pre Chorus]",
            "CHORUS",
            "Outro",
            "Verse II",
            "[Verse 2: Kendrick Lamar]",
            "[Chorus - Both]",
            "Chorus x2",
            "[Chorus (x3)]",
            "2x Chorus",
            "Chorus 2x",
            "[Drop]",
            "Solo:",
            "Hook 2",
        ] {
            assert!(is_section_label(l), "{l:?} is a label");
        }
    }

    #[test]
    fn lyrics_are_not_section_labels() {
        for l in [
            "The crowd threw me out for the kill",
            "Hook me up",
            "Chorus girls",
            "",
            "(oh oh oh)",
            "Verse after verse",
            "Drop",
            "Solo",
            "Remix2",
            "Chorusx2",
        ] {
            assert!(!is_section_label(l), "{l:?} is a lyric");
        }
    }
}
