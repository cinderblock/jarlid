//! LRCLIB's Lyricsfile: lyrics with what LRC can't hold — where each word starts, and where a
//! line ends. Spec: <https://github.com/tranxuanthang/lyricsfile/blob/main/SPECIFICATION.md>.
//!
//! LRCLIB stores a published Lyricsfile and derives the old fields from it: `syncedLyrics` gets
//! one `[mm:ss.xx]` line per Lyricsfile line, with no line ends, and `plainLyrics` the document's
//! `plain`. Players that only read those still get line-level lyrics; newer ones read the
//! Lyricsfile. So a pause has to be written twice to reach both: as the line's `end_ms`, and as
//! an empty line at that moment, which is how LRC has always cleared the display.
//!
//! LRCLIB parses with `serde_yaml` 0.9, as this does, and a Lyricsfile it can't parse is stored
//! with **no** legacy lyrics at all. [`build`] therefore checks its own output the way LRCLIB
//! will read it before it is published. See `plans/lyrics-timeline.md`.

use serde::{Deserialize, Serialize};

/// One line as the editor and the pane handle it: seconds, words without their spacing. An
/// empty `text` is a pause: the display clears from `start`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TimedLine {
    pub text: String,
    pub start: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<TimedWord>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TimedWord {
    pub text: String,
    pub start: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<f64>,
}

/// Whether `lines` hold anything LRC can't: word timings, or line ends.
pub fn beyond_lrc(lines: &[TimedLine]) -> bool {
    lines.iter().any(|l| !l.words.is_empty() || l.end.is_some())
}

// ---- the document, as written ---------------------------------------------------------------

const VERSION: &str = "1.0";

#[derive(Serialize, Deserialize, Debug)]
struct Document {
    version: String,
    metadata: Metadata,
    #[serde(default)]
    lines: Vec<Line>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plain: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct Metadata {
    title: String,
    artist: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    duration_ms: Option<i64>,
    #[serde(default)]
    instrumental: bool,
}

#[derive(Serialize, Deserialize, Debug)]
struct Line {
    #[serde(default)]
    text: String,
    start_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    end_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    words: Vec<Word>,
}

#[derive(Serialize, Deserialize, Debug)]
struct Word {
    /// With the whitespace that follows it, so the words joined are the line.
    text: String,
    start_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    end_ms: Option<i64>,
}

fn ms(seconds: f64) -> i64 {
    (seconds.max(0.0) * 1000.0).round() as i64
}

fn secs(ms: i64) -> f64 {
    ms as f64 / 1000.0
}

// ---- reading --------------------------------------------------------------------------------

/// The timed lines of a Lyricsfile, or `None` if it holds nothing LRC doesn't already (then the
/// record's `syncedLyrics` says it all), isn't version 1.0, or can't be read.
pub fn parse(yaml: &str) -> Option<Vec<TimedLine>> {
    let doc: Document = serde_yaml::from_str(yaml).ok()?;
    // The spec: readers must not treat an unknown version as 1.0.
    if doc.version != VERSION || doc.metadata.instrumental {
        return None;
    }
    let mut lines: Vec<TimedLine> = doc
        .lines
        .into_iter()
        .map(|l| {
            let words: Vec<TimedWord> = l
                .words
                .into_iter()
                .filter(|w| !w.text.trim().is_empty())
                .map(|w| TimedWord {
                    text: w.text.trim().to_string(),
                    start: secs(w.start_ms),
                    end: w.end_ms.map(secs),
                })
                .collect();
            let text = if words.is_empty() {
                l.text.trim().to_string()
            } else {
                words
                    .iter()
                    .map(|w| w.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            TimedLine {
                text,
                start: secs(l.start_ms),
                end: l.end_ms.map(secs),
                words,
            }
        })
        .collect();
    lines.sort_by(|a, b| a.start.total_cmp(&b.start));
    // LRCLIB writes a Lyricsfile for every record, and for one made from LRC every line "ends"
    // where the next begins. That says nothing LRC doesn't; only an end before the next line
    // starts is a pause.
    for i in 0..lines.len() {
        let next = lines.get(i + 1).map(|n| n.start);
        if let (Some(end), Some(next)) = (lines[i].end, next) {
            if end >= next {
                lines[i].end = None;
            }
        }
    }
    beyond_lrc(&lines).then_some(lines)
}

// ---- writing --------------------------------------------------------------------------------

/// The track a Lyricsfile describes, as LRCLIB files it.
pub struct Track<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub duration: f64,
}

/// A Lyricsfile for `lines`, ready to publish: checked to read back, the way LRCLIB reads it,
/// as exactly these lines. A line's `end` with no empty line at that moment gets one, so players
/// that only see the derived LRC still see the pause.
pub fn build(track: &Track, lines: &[TimedLine], plain: Option<&str>) -> Result<String, String> {
    let mut out: Vec<Line> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let words: Vec<Word> = l
            .words
            .iter()
            .enumerate()
            .map(|(k, w)| Word {
                text: if k + 1 < l.words.len() {
                    format!("{} ", w.text)
                } else {
                    w.text.clone()
                },
                start_ms: ms(w.start),
                end_ms: w.end.map(ms),
            })
            .collect();
        out.push(Line {
            text: l.text.clone(),
            start_ms: ms(l.start),
            end_ms: l.end.map(ms),
            words,
        });
        if let Some(end) = l.end.filter(|_| !l.text.trim().is_empty()) {
            let next = lines.get(i + 1);
            let cleared = next.is_some_and(|n| n.text.trim().is_empty() && ms(n.start) == ms(end));
            let before_next = next.is_none_or(|n| ms(end) < ms(n.start));
            if !cleared && before_next {
                out.push(Line {
                    text: String::new(),
                    start_ms: ms(end),
                    end_ms: None,
                    words: Vec::new(),
                });
            }
        }
    }
    let doc = Document {
        version: VERSION.into(),
        metadata: Metadata {
            title: track.title.trim().into(),
            artist: track.artist.trim().into(),
            album: Some(track.album.trim().to_string()).filter(|a| !a.is_empty()),
            duration_ms: (track.duration > 0.0).then(|| ms(track.duration)),
            instrumental: false,
        },
        lines: out,
        plain: plain
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(String::from),
    };
    let yaml = serde_yaml::to_string(&doc).map_err(|e| e.to_string())?;
    check(&yaml, &doc)?;
    Ok(yaml)
}

/// What LRCLIB will make of `yaml` for players that only read `syncedLyrics`: one LRC line per
/// Lyricsfile line, its words joined when it has words. Mirrors `parse_lyricsfile` in LRCLIB's
/// `server/src/lyricsfile.rs`.
pub fn derived_synced(yaml: &str) -> Result<String, String> {
    #[derive(Deserialize)]
    struct Doc {
        #[serde(default)]
        lines: Vec<L>,
    }
    #[derive(Deserialize)]
    struct L {
        text: Option<String>,
        start_ms: Option<i64>,
        #[serde(default)]
        words: Vec<W>,
    }
    #[derive(Deserialize)]
    struct W {
        text: Option<String>,
    }
    let doc: Doc = serde_yaml::from_str(yaml).map_err(|e| e.to_string())?;
    Ok(doc
        .lines
        .iter()
        .filter_map(|l| {
            let start = l.start_ms?;
            let text = if l.words.is_empty() {
                l.text.clone()?
            } else {
                l.words.iter().filter_map(|w| w.text.as_deref()).collect()
            };
            let (m, s, cs) = (start / 60_000, (start % 60_000) / 1000, (start % 1000) / 10);
            Some(format!("[{m:02}:{s:02}.{cs:02}]{text}"))
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

/// `yaml` must read back as `doc`, and give old players a line for every line.
fn check(yaml: &str, doc: &Document) -> Result<(), String> {
    let back: Document =
        serde_yaml::from_str(yaml).map_err(|e| format!("Lyricsfile doesn't read back: {e}"))?;
    let same = back.lines.len() == doc.lines.len()
        && back.lines.iter().zip(&doc.lines).all(|(a, b)| {
            a.text == b.text
                && a.start_ms == b.start_ms
                && a.end_ms == b.end_ms
                && a.words.len() == b.words.len()
        });
    if !same {
        return Err("Lyricsfile doesn't read back as the lines it was made from".into());
    }
    let derived = derived_synced(yaml)?;
    if derived.lines().count() != doc.lines.len() || doc.lines.is_empty() {
        return Err("Lyricsfile would give old players the wrong number of lines".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> Track<'static> {
        Track {
            title: "Kryptonite",
            artist: "3 Doors Down",
            album: "The Better Life",
            duration: 233.5,
        }
    }

    fn word(text: &str, start: f64) -> TimedWord {
        TimedWord {
            text: text.into(),
            start,
            end: None,
        }
    }

    fn lines() -> Vec<TimedLine> {
        vec![
            TimedLine {
                text: "I took a walk".into(),
                start: 19.5,
                end: Some(22.25),
                words: vec![
                    word("I", 19.5),
                    word("took", 19.8),
                    word("a", 20.1),
                    word("walk", 20.4),
                ],
            },
            TimedLine {
                text: "Around the world".into(),
                start: 23.0,
                end: None,
                words: vec![],
            },
        ]
    }

    #[test]
    fn builds_what_lrclib_reads_and_old_players_see_the_pause() {
        let yaml = build(&track(), &lines(), Some("I took a walk\nAround the world")).unwrap();
        assert_eq!(
            derived_synced(&yaml).unwrap(),
            "[00:19.50]I took a walk\n[00:22.25]\n[00:23.00]Around the world"
        );
        assert!(yaml.contains("version: '1.0'"), "{yaml}");
        assert!(yaml.contains("text: 'took '"), "{yaml}");
        assert!(yaml.contains("duration_ms: 233500"), "{yaml}");
    }

    #[test]
    fn reads_back_what_it_wrote() {
        let yaml = build(&track(), &lines(), None).unwrap();
        let back = parse(&yaml).unwrap();
        // The empty line written for old players comes back as a pause of its own, which says
        // where the line ended; the end itself, now redundant, is dropped.
        assert_eq!(back.len(), 3);
        assert_eq!(
            back[0],
            TimedLine {
                end: None,
                ..lines()[0].clone()
            }
        );
        assert_eq!(back[1].text, "");
        assert_eq!(back[1].start, 22.25);
        assert_eq!(back[2], lines()[1]);
    }

    #[test]
    fn an_existing_empty_line_at_the_end_is_not_doubled() {
        let mut l = lines();
        l.insert(
            1,
            TimedLine {
                text: String::new(),
                start: 22.25,
                end: None,
                words: vec![],
            },
        );
        let yaml = build(&track(), &l, None).unwrap();
        assert_eq!(derived_synced(&yaml).unwrap().lines().count(), 3);
    }

    #[test]
    fn awkward_text_survives_yaml() {
        let mut l = lines();
        l[1].text = "Don't: say \"no\" # or 'yes' - [ok] {x}".into();
        let yaml = build(&track(), &l, None).unwrap();
        assert_eq!(parse(&yaml).unwrap()[2].text, l[1].text);
        assert!(derived_synced(&yaml).unwrap().ends_with(&l[1].text));
    }

    #[test]
    fn plain_line_timing_is_left_to_lrc() {
        let only_lines = r#"
version: '1.0'
metadata: { title: T, artist: A }
lines:
  - { text: One, start_ms: 1000 }
  - { text: Two, start_ms: 2000 }
"#;
        assert_eq!(parse(only_lines), None);
    }

    /// As LRCLIB serves a record that was published as LRC (Blinding Lights, 2026-10-26).
    #[test]
    fn lines_that_end_where_the_next_begins_are_just_lrc() {
        let yaml = r#"
version: '1.0'
metadata:
  title: The Weeknd - Blinding Lights
  artist: The Weeknd
  duration_ms: 203000
  instrumental: false
lines:
- text: Yeah
  start_ms: 14150
  end_ms: 17730
- text: ''
  start_ms: 17730
  end_ms: 27850
- text: I've been tryna call
  start_ms: 27850
"#;
        assert_eq!(parse(yaml), None);
        // An end before the next line is a real pause.
        let paused = yaml.replace("end_ms: 27850", "end_ms: 20000");
        assert_eq!(parse(&paused).unwrap()[1].end, Some(20.0));
    }

    #[test]
    fn reads_lrcget_style_word_sync() {
        let yaml = r#"
version: '1.0'
metadata:
  title: T
  artist: A
lines:
- text: Hello world
  start_ms: 1200
  end_ms: 2800
  words:
  - text: 'Hello '
    start_ms: 1200
    end_ms: 1900
  - text: world
    start_ms: 1900
    end_ms: 2800
"#;
        let l = parse(yaml).unwrap();
        assert_eq!(l[0].text, "Hello world");
        assert_eq!(l[0].end, Some(2.8));
        assert_eq!(l[0].words[1].text, "world");
        assert_eq!(l[0].words[1].start, 1.9);
        assert_eq!(l[0].words[1].end, Some(2.8));
    }

    #[test]
    fn unknown_versions_are_not_read() {
        let yaml = "version: '2.0'\nmetadata: {title: T, artist: A}\nlines:\n- {text: X, start_ms: 1, end_ms: 2}\n";
        assert_eq!(parse(yaml), None);
    }
}
