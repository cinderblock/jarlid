//! LRC in and out. Mirrors `app/src/lrc.ts`, which is what will display the result.

/// One timed line.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub t: f64,
    pub text: String,
}

/// Every timestamped line of `lrc`, sorted by time. A line with several stamps appears once per
/// stamp; metadata tags like `[ar:…]` are not timestamps and are skipped.
pub fn parse(lrc: &str) -> Vec<Line> {
    let mut out = Vec::new();
    for raw in lrc.lines() {
        let mut rest = raw.trim();
        let mut stamps = Vec::new();
        while let Some(after) = rest.strip_prefix('[') {
            let Some(close) = after.find(']') else { break };
            match stamp(&after[..close]) {
                Some(t) => stamps.push(t),
                None => break,
            }
            rest = after[close + 1..].trim_start();
        }
        for t in stamps {
            out.push(Line {
                t,
                text: rest.trim().to_string(),
            });
        }
    }
    // Stable, so lines sharing a stamp keep their order.
    out.sort_by(|a, b| a.t.total_cmp(&b.t));
    out
}

/// `mm:ss`, `mm:ss.xx` or `mm:ss:xx` to seconds.
fn stamp(s: &str) -> Option<f64> {
    let (min, sec) = s.split_once(':')?;
    let min: u32 = min.parse().ok()?;
    let (sec, frac) = match sec.find(['.', ':']) {
        Some(i) => (&sec[..i], &sec[i + 1..]),
        None => (sec, ""),
    };
    let sec: u32 = sec.parse().ok()?;
    let frac = if frac.is_empty() {
        0.0
    } else {
        let digits: String = frac.chars().take(3).collect();
        digits.parse::<u32>().ok()? as f64 / 10f64.powi(digits.len() as i32)
    };
    Some(min as f64 * 60.0 + sec as f64 + frac)
}

/// LRC text for `lines`, centisecond stamps.
pub fn format(lines: &[Line]) -> String {
    let mut out = String::new();
    for line in lines {
        let cs = (line.t.max(0.0) * 100.0).round() as u64;
        out.push_str(&format!(
            "[{:02}:{:02}.{:02}] {}\n",
            cs / 6000,
            cs / 100 % 60,
            cs % 100,
            line.text
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stamps_and_skips_tags() {
        let lines = parse("[ar:Someone]\n[00:12.50] Hello\n[01:02.3][00:05] Twice\n[00:20.00]\n");
        let got: Vec<(f64, &str)> = lines.iter().map(|l| (l.t, l.text.as_str())).collect();
        assert_eq!(
            got,
            [(5.0, "Twice"), (12.5, "Hello"), (20.0, ""), (62.3, "Twice")]
        );
    }

    #[test]
    fn round_trips() {
        let lines = vec![Line {
            t: 75.25,
            text: "a line".into(),
        }];
        assert_eq!(parse(&format(&lines)), lines);
    }
}
