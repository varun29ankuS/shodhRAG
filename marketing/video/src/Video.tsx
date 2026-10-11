import React from "react";
import { AbsoluteFill, Html5Audio, Sequence, interpolate, staticFile, useCurrentFrame, useVideoConfig } from "remotion";
import { Scene, toneOf } from "./scenes";
import { calm, color } from "./theme";
import type { Slot } from "./timeline";

const MUSIC_LEVEL = 0.32;
const GROUND_TURN = 36;

/** The ground: dark for the opening, eased to light as scene 3 begins. */
function useGround(list: Slot[]): string {
  const frame = useCurrentFrame();
  const firstLight = list.find((s) => toneOf(s.scene) === "light");
  if (!firstLight || toneOf(list[0].scene) === "light") return color.light.ground;
  const t = interpolate(frame, [firstLight.from, firstLight.from + GROUND_TURN], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: calm,
  });
  return mixHex(color.dark.ground, color.light.ground, t);
}

function mixHex(a: string, b: string, t: number): string {
  const channel = (hex: string, i: number) => parseInt(hex.slice(1 + i * 2, 3 + i * 2), 16);
  const out = [0, 1, 2].map((i) => Math.round(channel(a, i) + (channel(b, i) - channel(a, i)) * t));
  return `rgb(${out.join(",")})`;
}

export const ShodhVideo: React.FC<{ list: Slot[]; music: boolean }> = ({ list, music }) => {
  const { durationInFrames, fps } = useVideoConfig();
  const ground = useGround(list);
  return (
    <AbsoluteFill style={{ background: ground }}>
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
