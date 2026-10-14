#!/usr/bin/env bash
# Spike: run one speech-model configuration over corpus tracks and record wall time.
#
#   [THREADS=n] run-asr.sh <config> <track-dir>...
#
# Configs are named <engine>-<backend>-<model>. Output: $SPIKE/results/<config>/<track>.{json,txt}
# and a line per track in $SPIKE/results/<config>/times.tsv (track, audio seconds, wall seconds).
# Run under the CPU broker claiming as many slots as THREADS (default 4).
set -euo pipefail
THREADS="${THREADS:-4}"
SPIKE="${SPIKE:-$LOCALAPPDATA/jarlid-autosync-spike}"
CPU="$SPIKE/dl/whisper-cpu/Release"
GPU="$SPIKE/src/whisper.cpp/build-vulkan/bin/Release"
M="$SPIKE/models"
config="$1"; shift
out="$SPIKE/results/$config"; mkdir -p "$out"

case "$config" in
  whisper-cpu-*|parakeet-cpu-*) bin="$CPU" ;;
  whisper-vk-*|parakeet-vk-*) bin="$GPU" ;;
  whistle-*) bin="" ;;
  *) echo "unknown config $config" >&2; exit 2 ;;
esac

for dir in "$@"; do
  track="$(basename "$dir")"
  wav="$dir/audio.wav"
  secs=$(( ($(stat -c %s "$wav") - 44) / 32000 ))
  start=$(date +%s%N)
  case "$config" in
    whisper-*-base)  "$bin/whisper-cli.exe" -t "$THREADS" -l en -m "$M/ggml-base-q5_1.bin" --dtw base -ojf -of "$out/$track" -np "$wav" ;;
    whisper-*-small) "$bin/whisper-cli.exe" -t "$THREADS" -l en -m "$M/ggml-small-q5_1.bin" --dtw small -ojf -of "$out/$track" -np "$wav" ;;
    whisper-*-turbo) "$bin/whisper-cli.exe" -t "$THREADS" -l en -m "$M/ggml-large-v3-turbo-q5_0.bin" --dtw large.v3.turbo -ojf -of "$out/$track" -np "$wav" ;;
    parakeet-*-q8)   "$bin/parakeet-cli.exe" -t "$THREADS" -m "$M/ggml-parakeet-tdt-0.6b-v3-q8_0.bin" -ps -np -f "$wav" > "$out/$track.txt" 2>&1 ;;
    parakeet-*-f16)  "$bin/parakeet-cli.exe" -t "$THREADS" -m "$M/ggml-parakeet-tdt-0.6b-v3-f16.bin" -ps -np -f "$wav" > "$out/$track.txt" 2>&1 ;;
    # Fixed decode: no conditioning on previous text (stops repetition loops), flash attention
    # off (whisper.cpp silently disables DTW with it on), Silero VAD so instrumental stretches
    # aren't transcribed at all.
    whisper-*-base2)  "$bin/whisper-cli.exe" -t "$THREADS" -l en -m "$M/ggml-base-q5_1.bin" --dtw base -nfa -mc 0 --vad -vm "$M/ggml-silero-v5.1.2.bin" -ojf -of "$out/$track" -np "$wav" ;;
    whisper-*-small2) "$bin/whisper-cli.exe" -t "$THREADS" -l en -m "$M/ggml-small-q5_1.bin" --dtw small -nfa -mc 0 --vad -vm "$M/ggml-silero-v5.1.2.bin" -ojf -of "$out/$track" -np "$wav" ;;
    # Parakeet over overlapping windows (see chunk.py); one process, one model load, many files.
    # Named parakeet-<backend>-<quant>w, where <quant> is the model file's suffix (q4_0, q8_0, f16).
    parakeet-*w)
      chunks="$SPIKE/chunks/$track"; python -I "$(dirname "$0")/chunk.py" "$wav" "$chunks" 30 6
      quant="${config##*-}"; quant="${quant%w}"
      "$bin/parakeet-cli.exe" -t "$THREADS" -m "$M/ggml-parakeet-tdt-0.6b-v3-$quant.bin" -ps "$chunks"/*.wav > "$out/$track.txt" 2>&1 ;;
    # Whistle (Cactus Compute), CPU only; the lyric's own words as keyword bias.
    whistle-cpu-kwwords)
      python -I "$(dirname "$0")/keywords.py" "$dir/plain.txt" > "$out/$track.kw"
      "$SPIKE/dl/needle-win64/needle.exe" --model "$M/whistle.cact" --audio "$wav" --audio-language en         --audio-word-timestamps --audio-keywords "$out/$track.kw" --threads "$THREADS" > "$out/$track.json" 2> "$out/$track.err" ;;
    *) echo "unknown config $config" >&2; exit 2 ;;
  esac
  wall=$(awk "BEGIN{printf \"%.1f\", ($(date +%s%N) - $start) / 1e9}")
  printf '%s\t%s\t%s\n' "$track" "$secs" "$wall" | tee -a "$out/times.tsv"
done
