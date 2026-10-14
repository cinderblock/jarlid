"""Spike: drive jarlid-autosync over its pipe protocol and save what it heard for bench.rs.

    drive-sidecar.py <exe> <model> <out-dir> <gpu:0|1> <threads> <track-dir>...

Writes <out-dir>/<track>.json in the {"words":[{"word","start","end","probability"}]} shape the
bench already reads, and a times.tsv like run-asr.sh. One sidecar process, one model load, every
track in turn — the way the app will use it.
"""
import json, os, subprocess, sys, time

exe, model, out, gpu, threads, tracks = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4] == "1", int(sys.argv[5]), sys.argv[6:]
os.makedirs(out, exist_ok=True)
p = subprocess.Popen([exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=open(os.path.join(out, "stderr.log"), "w"), text=True, encoding="utf-8")

def send(o):
    p.stdin.write(json.dumps(o) + "\n"); p.stdin.flush()

def event():
    line = p.stdout.readline()
    if not line:
        sys.exit("sidecar exited")
    return json.loads(line)

print(event())
send({"op": "load", "model": model, "gpu": gpu, "threads": threads})
print(event())
for d in tracks:
    track = os.path.basename(os.path.normpath(d))
    wav = os.path.join(d, "audio.wav")
    secs = (os.path.getsize(wav) - 44) / 32000
    began = time.time()
    send({"op": "transcribe", "id": track, "wav": wav})
    words = []
    while True:
        e = event()
        if e["ev"] == "words":
            words += [{"word": w["text"], "start": w["start"], "end": w["end"], "probability": w["p"]} for w in e["words"]]
        elif e["ev"] in ("done", "error", "cancelled"):
            if e["ev"] != "done":
                print(track, e)
            break
    words.sort(key=lambda w: w["start"])
    json.dump({"words": words}, open(os.path.join(out, track + ".json"), "w", encoding="utf-8"))
    wall = time.time() - began
    line = f"{track}\t{secs:.0f}\t{wall:.1f}"
    print(line)
    open(os.path.join(out, "times.tsv"), "a").write(line + "\n")
send({"op": "quit"})
p.wait()
