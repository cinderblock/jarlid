r"""One-off watch: is LRCLIB record 39914973 (Emei - Molly) still serving what was published?

    python -I scripts/check-molly-lrclib.py

Published 2026-11-09 6:00 PM PST (02:00Z) at Cameron's request: the word-timed Lyricsfile in
molly-lrclib-published.json beside this script (the exact publish body). Compares both reads
the app makes (by id, and the exact track lookup) against it. Appends one line to
%LOCALAPPDATA%\com.camer.pandora-desktop\molly-watch\check.log and prints it.
Exit 0 = ours, 1 = reverted / changed, 2 = could not tell. A revert is reported, never
republished without asking.
"""
import datetime, json, os, sys, time, urllib.parse, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
LOG = os.path.join(os.environ["LOCALAPPDATA"], "com.camer.pandora-desktop", "molly-watch", "check.log")
UA = "PandoraDesktop (personal; https://github.com/cinderblock/pandora-desktop)"
Q = dict(track_name="Molly", artist_name="Emei", album_name="Molly", duration="192")


def get(url):
    err = None
    for _ in range(6):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=30) as r:
                return json.loads(r.read())
        except Exception as e:
            err = e
            time.sleep(10)
    raise err


def verdict(rec, ours):
    if rec.get("lyricsfile") == ours["lyricsfile"] and rec.get("plainLyrics") == ours["plainLyrics"]:
        return "ours"
    s = rec.get("syncedLyrics") or ""
    return (f"OTHER (id {rec.get('id')}, {len(s.splitlines())} lines, "
            f"word sync {rec.get('hasWordSync')}, 'his mind' x{s.count('his mind')})")


def main():
    now = datetime.datetime.now().astimezone()
    zone = "PDT" if now.dst() else "PST"  # %Z is "Pacific Standard Time" on Windows
    stamp = f"{now:%Y-%m-%d %I:%M %p} {zone} ({now.astimezone(datetime.timezone.utc):%H:%MZ})"
    try:
        with open(os.path.join(HERE, "molly-lrclib-published.json"), encoding="utf-8") as f:
            ours = json.load(f)
        by_id = verdict(get("https://lrclib.net/api/get/39914973"), ours)
        lookup = verdict(get("https://lrclib.net/api/get?" + urllib.parse.urlencode(Q)), ours)
        code = 0 if by_id == lookup == "ours" else 1
        line = f"{stamp}  by id: {by_id}  lookup: {lookup}"
    except Exception as e:
        code, line = 2, f"{stamp}  could not tell: {e}"
    os.makedirs(os.path.dirname(LOG), exist_ok=True)
    with open(LOG, "a", encoding="utf-8") as f:
        f.write(line + "\n")
    print(line)
    sys.exit(code)


main()
