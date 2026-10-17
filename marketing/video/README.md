# Shodh launch video

Remotion project for the launch video: on Shodh orange, an animated chapter title on the left (number, headline, one line of detail and a drawn-on illustration) and the real app on the right, cut to the music's bars. The brand mark stays in the top left throughout. No voice.

| Composition | Size | Length | Output |
|---|---|---|---|
| `ShodhLaunch` | 1920×1080 @ 30 fps | 1:47, 15 chapters and a close | `npm run render` → `out/shodh-launch.mp4` (H.264, CRF 18) |
| `ShodhLoop` | 960 px wide GIF, 15 fps | about 12 s: answer, citation, grounding | `npm run render:gif` → `out/shodh-loop.gif` |

## Setup

```sh
npm ci
npm run studio   # preview in the browser
```

The first render downloads Remotion's headless browser. After that, rendering makes no network calls; the font (Geist) is bundled from `node_modules`.

## Music

The track is "Study Music" by The_Mountain ([Pixabay 602501](https://pixabay.com/music/)), under the Pixabay Content License, which allows its use in the video but not redistributing the file, so neither the source nor the edit is committed. Download it, then run:

```sh
npm run beats -- path/to/the_mountain-study-music-602501.mp3
```

`scripts/beats.py` (needs `pip install librosa` and ffmpeg) fits a constant tempo grid to the track, cuts it from two sections joined on matching bars (0:00–1:11.9, then 1:56.3 to a two-bar fade), and writes `public/music/track.mp3` and `src/beats.json`. Every cut, reveal and pulse sits on that grid: `src/cut.ts` lists the shots by the bar each starts on.

## Footage

Every shot except the close shows real footage of the app. A shot with no footage fails the render instead of showing a stand-in.

The footage is not committed (it is large and re-recorded for each release). Record it from the release build in a separate demo profile (`SHODH_DATA_DIR`) with a library of public papers, then:

1. Put one clip per shot in `public/footage/<shot>/clip.mp4`. The shot ids are in `src/cut.ts` (every shot but `close`). Each clip must be at least as long as its shot; speed it up when cutting rather than trimming the action. Capture at 16:9, at least 1920×1080, so the punch-ins stay sharp. PNG/JPG stills or a numbered PNG sequence also work (see `public/footage.json`).
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
