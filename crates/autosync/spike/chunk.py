"""Spike: cut a 16 kHz mono WAV into overlapping windows for a speech model.

    chunk.py <in.wav> <out-dir> <window-seconds> <overlap-seconds>

Writes w0000_t0.000.wav, w0001_t24.000.wav, ... — the start time is in the name so the scorer
can put each window's words back on the track's clock and drop the duplicated overlap.
"""
import os, sys, wave

src, out, window, overlap = sys.argv[1], sys.argv[2], float(sys.argv[3]), float(sys.argv[4])
os.makedirs(out, exist_ok=True)
for f in os.listdir(out):
    os.remove(os.path.join(out, f))
with wave.open(src, "rb") as w:
    rate, frames = w.getframerate(), w.readframes(w.getnframes())
step, size, n = int((window - overlap) * rate), int(window * rate), len(frames) // 2
i = 0
for start in range(0, max(n - int(overlap * rate), 1), step):
    with wave.open(os.path.join(out, f"w{i:04d}_t{start / rate:.3f}.wav"), "wb") as o:
        o.setnchannels(1); o.setsampwidth(2); o.setframerate(rate)
        o.writeframes(frames[start * 2:(start + size) * 2])
    i += 1
