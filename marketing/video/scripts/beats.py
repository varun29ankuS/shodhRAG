"""Edits the music for the video and writes its beat grid.

Usage: python scripts/beats.py SOURCE.mp3   (needs librosa and ffmpeg on PATH)

1. Fits a constant tempo grid to the track: librosa's beat tracker, a linear
   fit through its beats, then a small search around that fit for the grid
   that lands best on the onsets. A tracker's beats jitter by its hop size;
   a fitted grid does not drift over two minutes.
2. Cuts the track from two sections joined on matching bars (EDIT below, in
   grid beats): the end of the first section and the start of the second
   are both section changes on the same beat of the bar, so the join keeps
   the beat spacing and the bar position. The last two bars fade out.
3. Writes public/music/track.mp3 and src/beats.json (the edited track's grid,
   with the beat that starts each bar).
"""
import json
import shutil
import subprocess
import sys
from pathlib import Path

import librosa
import numpy as np

ROOT = Path(__file__).resolve().parent.parent
OUT_TRACK = ROOT / "public" / "music" / "track.mp3"
OUT_BEATS = ROOT / "src" / "beats.json"
SOURCE = "Study Music (The_Mountain, Pixabay 602501)"

# EDIT, in grid beats: keep [0, A_END) and [B_START, B_END); bars start on BAR_PHASE.
A_END = 153
B_START = 249
B_END = 357
BAR_PHASE = 1


def fit_grid(y: np.ndarray, sr: int) -> tuple[float, float]:
    # The tracker runs at 22.05 kHz, where it holds this track's 129.6 BPM; at
    # full rate it can lock onto a slower subdivision. The fit is refined on
    # full-rate onsets below.
    _, beats = librosa.beat.beat_track(y=librosa.resample(y, orig_sr=sr, target_sr=22050), sr=22050, units="time")
    index = np.arange(len(beats))
    period, start = np.polyfit(index, beats, 1)
    close = np.abs(beats - (start + period * index)) < 0.06
    period, start = np.polyfit(index[close], beats[close], 1)
    hop = 128
    onsets = librosa.onset.onset_strength(y=y, sr=sr, hop_length=hop)
    duration = len(y) / sr

    def score(s: float, p: float) -> float:
        grid = s + p * np.arange(int((duration - s) / p))
        frames = librosa.time_to_frames(grid, sr=sr, hop_length=hop)
        return float(onsets[frames[frames < len(onsets)]].mean())

    _, start, period = max(
        (score(start + ds, period * (1 + dp)), start + ds, period * (1 + dp))
        for ds in np.linspace(-0.03, 0.03, 31)
        for dp in np.linspace(-0.002, 0.002, 21)
    )
    return start, period


def main() -> None:
    source = Path(sys.argv[1])
    ffmpeg = shutil.which("ffmpeg")
    if ffmpeg is None:
        raise SystemExit("ffmpeg is not on PATH")
    y, sr = librosa.load(source, sr=44100, mono=True)
    start, period = fit_grid(y, sr)
    a_end = start + A_END * period
    b_start = start + B_START * period
    b_end = start + B_END * period
    fade = 8 * period
    graph = (
        f"[0:a]atrim=0:{a_end:.4f},asetpts=PTS-STARTPTS,afade=t=out:st={a_end - 0.012:.4f}:d=0.012[a];"
        f"[0:a]atrim={b_start:.4f}:{b_end:.4f},asetpts=PTS-STARTPTS,afade=t=in:st=0:d=0.012,"
        f"afade=t=out:st={b_end - b_start - fade:.4f}:d={fade:.4f}[b];[a][b]concat=n=2:v=0:a=1[o]"
    )
    OUT_TRACK.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [ffmpeg, "-y", "-loglevel", "error", "-i", str(source), "-filter_complex", graph,
         "-map", "[o]", "-c:a", "libmp3lame", "-b:a", "256k", str(OUT_TRACK)],
        check=True,
    )
    total = a_end + (b_end - b_start)
    beats = [round(start + j * period, 4) for j in range(int((total - start) / period) + 1)]
    OUT_BEATS.write_text(json.dumps({
        "source": f"{SOURCE}, edited: 0:00-{a_end:.1f} s + {b_start:.1f}-{b_end:.1f} s",
        "tempo": round(60 / period, 3),
        "barPhase": BAR_PHASE,
        "beats": beats,
    }))
    print(f"{OUT_TRACK}: {total:.2f} s; {len(beats)} beats at {60 / period:.2f} BPM")


if __name__ == "__main__":
    main()
