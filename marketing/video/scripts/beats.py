"""Writes src/beats.json: the beat times of public/music/track.mp3.

Usage: python scripts/beats.py   (needs librosa: pip install librosa)

Every cut, caption and pulse in the video is placed on these beats, so run it
again whenever the track changes, then check the bar phase in src/beats.ts.
"""
import json
from pathlib import Path

import librosa
import numpy as np

ROOT = Path(__file__).resolve().parent.parent
TRACK = ROOT / "public" / "music" / "track.mp3"
OUT = ROOT / "src" / "beats.json"
SOURCE = "Future Tech Corporate Commercial (PaulYudin, Pixabay 322264)"


def main() -> None:
    y, sr = librosa.load(TRACK, sr=22050, mono=True)
    tempo, beats = librosa.beat.beat_track(y=y, sr=sr, units="time")
    data = {
        "source": SOURCE,
        "tempo": round(float(np.atleast_1d(tempo)[0]), 3),
        "beats": [round(float(b), 3) for b in beats],
    }
    OUT.write_text(json.dumps(data))
    print(f"{OUT}: {len(beats)} beats at {data['tempo']} BPM")


if __name__ == "__main__":
    main()
