import React from "react";
import { AbsoluteFill, Html5Audio, Sequence, interpolate, staticFile, useVideoConfig } from "remotion";
import { Scene } from "./scenes";
import { color } from "./theme";
import type { Slot } from "./timeline";

const MUSIC_LEVEL = 0.32;

export const ShodhVideo: React.FC<{ list: Slot[]; music: boolean }> = ({ list, music }) => {
  const { durationInFrames, fps } = useVideoConfig();
  return (
    <AbsoluteFill style={{ background: color.light.ground }}>
      {list.map((slot) => (
        <Sequence key={slot.scene} from={slot.from} durationInFrames={slot.durationInFrames} name={`Scene ${slot.scene}`}>
          <Scene scene={slot.scene} />
        </Sequence>
      ))}
      {music ? (
        <Html5Audio
          src={staticFile("music/shodh-ambient.wav")}
          volume={(f) =>
            MUSIC_LEVEL *
            interpolate(f, [durationInFrames - 4 * fps, durationInFrames], [1, 0], {
              extrapolateLeft: "clamp",
              extrapolateRight: "clamp",
            })
          }
        />
      ) : null}
    </AbsoluteFill>
  );
};
