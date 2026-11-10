"""One-off watch: is LRCLIB record 2785399 still serving the published mix?

    python -I scripts/check-dopamine-lrclib.py

Compares both reads the app makes (by id, and the exact track lookup) against
mix-lyrics-dopamine-published.json beside this script: the text Cameron reviewed and published.
Appends one line to %LOCALAPPDATA%\\com.camer.pandora-desktop\\dopamine-watch\\check.log and
prints it. Exit 0 = ours, 1 = reverted / changed, 2 = could not tell.
"""
import datetime, json, os, sys, time, urllib.parse, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
LOG = os.path.join(os.environ["LOCALAPPDATA"], "com.camer.pandora-desktop", "dopamine-watch", "check.log")
UA = "PandoraDesktop (personal; https://github.com/cinderblock/pandora-desktop)"
Q = dict(track_name="dopamine (French + English Version)", artist_name="Madelline",
         album_name="dopamine (French + English Version)", duration="165")


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
    s = (rec.get("syncedLyrics") or "").strip()
    if s == ours:
        return "ours"
    return f"OTHER ({len(s.splitlines())} lines, {'has English' if 'awake' in s else 'French only'})"


def main():
    now = datetime.datetime.now().astimezone()
    stamp = f"{now:%Y-%m-%d %I:%M %p %Z} ({now.astimezone(datetime.timezone.utc):%H:%MZ})"
    try:
        with open(os.path.join(HERE, "mix-lyrics-dopamine-published.json"), encoding="utf-8") as f:
            ours = json.load(f)["synced"].strip()
        by_id = verdict(get("https://lrclib.net/api/get/2785399"), ours)
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
