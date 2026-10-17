# Shodh launch video

Remotion project for the launch video: real app footage on Shodh orange, cut to the music's beats. Captions only, no voice.

| Composition | Size | Length | Output |
|---|---|---|---|
| `ShodhLaunch` | 1920×1080 @ 30 fps | 40 s, 8 shots | `npm run render` → `out/shodh-launch.mp4` (H.264, CRF 18) |
| `ShodhLoop` | 960 px wide GIF, 15 fps | about 12 s: answer, citation, grounding | `npm run render:gif` → `out/shodh-loop.gif` |

## Setup

```sh
npm ci
npm run studio   # preview in the browser
```

The first render downloads Remotion's headless browser. After that, rendering makes no network calls; the font (Geist) is bundled from `node_modules`.

## Music

The track is "Future Tech Corporate Commercial" by PaulYudin ([Pixabay 322264](https://pixabay.com/music/upbeat-future-tech-corporate-commercial-322264/), Pixabay Content License). The licence allows its use in the video but not redistributing the file, so it is not committed. Download it and save it as `public/music/track.mp3`.

The edit follows the track's beats, not a fixed grid: `src/beats.json` holds the beat times, written by `npm run beats` (`scripts/beats.py`, needs `pip install librosa`). `src/beats.ts` treats beats 3, 7, 11, … as bar starts, where the kick is strongest and the chords change. `src/cut.ts` lists the shots by the beat each starts on; every cut falls on a bar start. The video ends at 40 s, a bar after the close's last hit, where the track's first section ends. If the track changes, run `npm run beats`, check the bar phase, and adjust the shots.

## Footage

Every shot except the close shows real footage of the app. A shot with no footage fails the render instead of showing a stand-in.

The footage is not committed (it is large and re-recorded for each release). Record it from the release build in a separate demo profile (`SHODH_DATA_DIR`) with a library of public papers, then:

1. Put one clip per shot in `public/footage/<shot>/clip.mp4`. The shot ids are in `src/cut.ts`. Each clip must be at least as long as its shot; speed it up when cutting rather than trimming the action. Capture at 16:9, at least 1920×1080, so the punch-ins stay sharp. PNG/JPG stills or a numbered PNG sequence also work (see `public/footage.json`).
2. Set the camera in `public/footage.json` under `"keyframes"`:
   - `at` is the point in the shot, from 0 to 1;
   - `focus` is the area in footage coordinates (0–1) that the zoom centres on;
   - `scale` is the zoom at that point.

   The camera moves with a quick, hard-settling ease between keyframes, so keep the same focus across two keyframes to hold, and change it to punch in.

No code changes are needed. Re-render to see the result.

## Notes

- `out/`, `public/footage/` and `public/music/` are not committed.
- The GIF must stay at or under 8 MB. With real footage, check its size after rendering. If it is too big, raise `--every-nth-frame` or lower `--scale` in `render:gif`.

## License

Remotion is source-available under the [Remotion License](https://www.remotion.dev/license). It is free for individuals and for companies with up to 3 employees; larger for-profit companies need a company license.
