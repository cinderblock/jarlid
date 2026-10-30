"""One-off: mix two timed lyric files for a song that switches language line by line.

    mix-lyrics.py <grid.tsv> <audio.wav> <sidecar.exe> <model.bin> <out-dir> [--hear] [--record <file>]

LRCLIB has each language whole, and the two as a pair, but nothing for a track that alternates.
Both languages share one timing, so all that is missing is which one is sung on each line, and
the speech engine (Parakeet v3, which knows both) can say. Nothing here decides anything: every
line goes to a person, and the output is a local edit the app's editor reviews line by line.

grid.tsv has `time <tab> first language <tab> second language`, one line each; leave a language
empty for a line that only has the other. --hear runs the engine, once over the whole song and
once over each line on its own (a short clip has no earlier language to lean towards); without
it the last run's words are used. Writes verdicts.tsv, and mixed.lrc with the picks. --record
takes a file from the app's lyrics cache and writes override.json: the mix as a half-reviewed
draft of that record, every line to tick, the lines marked CHECK underlined. Copied into
lyrics-overrides under the same name, it opens in the editor, which won't publish until each
line is ticked.

Stdlib only; run it with `python -I`.
"""
import difflib, json, os, re, subprocess, sys, unicodedata, wave

LEAD = 0.25  # Heard words start a little either side of a line's time.
CLEAR = 0.12  # How far apart two scores must be before a line is called.
HEARD = 0.6  # And how well the winner must have been heard: 0.46 to 0.32 is two bad guesses.
ALIKE = 0.75  # Lines this close (Ma / My) differ by a word or less, which no score can settle.


def stamp(t):
    return f"[{int(t // 60):02}:{t % 60:05.2f}]"


def seconds(s):
    m, r = s.split(":")
    return int(m) * 60 + float(r)


def read_grid(path):
    rows = []
    for line in open(path, encoding="utf-8"):
        line = line.rstrip("\r\n")
        if line and not line.startswith("#"):
            t, a, b = line.split("\t")
            rows.append({"t": seconds(t), "a": a.strip(), "b": b.strip()})
    return rows


def fold(s):
    """Letters only, no accents: what two spellings of one sound most often share."""
    s = unicodedata.normalize("NFKD", s.lower())
    return re.sub(r"[^a-z]", "", s)


def sounds(s):
    """A crude ear: the same sound spelled in French or in English comes out alike. The engine
    writes sung French as English words ("two squash" for "tout ce que"), so spelling alone
    undersells it."""
    s = fold(s)
    for a, b in [("eau", "o"), ("aux", "o"), ("ou", "u"), ("oo", "u"), ("ch", "s"), ("sh", "s"), ("qu", "k"),
                 ("ck", "k"), ("ph", "f"), ("th", "t"), ("ai", "e"), ("ei", "e"), ("ea", "e"), ("ee", "e"),
                 ("c", "k"), ("y", "i"), ("z", "s"), ("w", "v")]:
        s = s.replace(a, b)
    return re.sub(r"(.)\1+", r"\1", s)


def closeness(heard, text):
    """How much of `text` was heard, 0-1: the best run of heard words, so words from the lines
    either side don't count against it."""
    if not heard or not fold(text):
        return 0.0
    best = 0.0
    for key in (fold, sounds):
        target = key(text)
        for i in range(len(heard)):
            for j in range(i + 1, len(heard) + 1):
                run = key(" ".join(heard[i:j]))
                if len(run) > 2 * len(target):
                    break
                best = max(best, difflib.SequenceMatcher(None, run, target, autojunk=False).ratio())
    return best


def hear_all(wav_path, exe, model, out):
    """The engine over the whole song, and over each line's clip, through its pipe protocol."""
    rows = read_grid(sys.argv[1])
    w = wave.open(wav_path, "rb")
    rate, total = w.getframerate(), w.getnframes()
    pcm = w.readframes(total)
    clips = os.path.join(out, "clips")
    os.makedirs(clips, exist_ok=True)
    jobs = [("song", wav_path)]
    for i, r in enumerate(rows):
        end = rows[i + 1]["t"] if i + 1 < len(rows) else total / rate
        a = max(0, int((r["t"] - LEAD) * rate))
        b = min(total, int((end + LEAD) * rate))
        path = os.path.join(clips, f"{i:03}.wav")
        with wave.open(path, "wb") as c:
            c.setnchannels(1)
            c.setsampwidth(2)
            c.setframerate(rate)
            c.writeframes(pcm[a * 2 : b * 2])
        jobs.append((str(i), path))

    p = subprocess.Popen([exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                         text=True, encoding="utf-8")

    def send(o):
        p.stdin.write(json.dumps(o) + "\n")
        p.stdin.flush()

    def event():
        line = p.stdout.readline()
        if not line:
            sys.exit("sidecar exited")
        return json.loads(line)

    event()
    send({"op": "load", "model": model, "gpu": False, "threads": 4})
    event()
    heard = {}
    for name, path in jobs:
        send({"op": "transcribe", "id": name, "wav": path})
        words = []
        while True:
            e = event()
            if e["ev"] == "words":
                words += e["words"]
            elif e["ev"] == "done":
                break
            elif e["ev"] in ("error", "cancelled"):
                print(name, e)
                break
        heard[name] = sorted(words, key=lambda x: x["start"])
    send({"op": "quit"})
    p.wait()
    json.dump(heard, open(os.path.join(out, "heard.json"), "w", encoding="utf-8"), ensure_ascii=False)


def alike(a, b):
    return difflib.SequenceMatcher(None, fold(a), fold(b), autojunk=False).ratio() >= ALIKE


def draft_of(record, picked):
    """The picks as the editor's draft: `Lyrics` in lyrics.rs, one review entry per LRC line."""
    with open(record, encoding="utf-8") as f:
        cached = json.load(f)
    lyrics = {k: cached.get(k) for k in ("id", "trackName", "artistName", "albumName", "duration")}
    lyrics.update(
        synced="\n".join(f"{stamp(t)} {text}" for t, text, _ in picked),
        plain="\n".join(text for _, text, _ in picked),
        source="local edit",
        overridden=True,
        draft={
            "doubt": [[] if sure else list(range(len(text.split()))) for _, text, sure in picked],
            "reviewed": [False] * len(picked),
        },
    )
    return lyrics


def main():
    grid, wav_path, exe, model, out = sys.argv[1:6]
    if "--hear" in sys.argv:
        hear_all(wav_path, exe, model, out)
    heard = json.load(open(os.path.join(out, "heard.json"), encoding="utf-8"))
    rows = read_grid(grid)
    song = heard["song"]
    picked = []
    last = "A"  # The language being sung, for lines that can't say for themselves.
    with open(os.path.join(out, "verdicts.tsv"), "w", encoding="utf-8") as v, \
         open(os.path.join(out, "mixed.lrc"), "w", encoding="utf-8", newline="\n") as lrc:
        v.write("time\tpick\tsure\tA\tB\tin song\tin clip\tfirst\tsecond\n")
        for i, r in enumerate(rows):
            end = rows[i + 1]["t"] if i + 1 < len(rows) else r["t"] + 5
            in_song = [w["text"] for w in song if w["end"] > r["t"] - LEAD and w["start"] < end + LEAD]
            in_clip = [w["text"] for w in heard.get(str(i), [])]
            a = max(closeness(in_song, r["a"]), closeness(in_clip, r["a"]))
            b = max(closeness(in_song, r["b"]), closeness(in_clip, r["b"]))
            if not r["a"] or not r["b"] or fold(r["a"]) == fold(r["b"]):
                pick, sure = "same", True
            elif alike(r["a"], r["b"]):
                pick, sure = last, False
            elif abs(a - b) >= CLEAR and max(a, b) >= HEARD:
                pick, sure = ("A" if a > b else "B"), True
                last = pick
            else:
                # Not enough to go on: the record this came from, for a person to check.
                pick, sure = "A", False
                last = pick
            text = r["b"] if pick == "B" else (r["a"] or r["b"])
            picked.append((r["t"], text, sure))
            lrc.write(f"{stamp(r['t'])} {text}\n")
            v.write(f"{stamp(r['t'])}\t{pick}\t{'yes' if sure else 'CHECK'}\t{a:.2f}\t{b:.2f}\t"
                    f"{' '.join(in_song)}\t{' '.join(in_clip)}\t{r['a']}\t{r['b']}\n")
    if "--record" in sys.argv:
        record = sys.argv[sys.argv.index("--record") + 1]
        with open(os.path.join(out, "override.json"), "w", encoding="utf-8") as o:
            json.dump(draft_of(record, picked), o, ensure_ascii=False, indent=2)


main()
