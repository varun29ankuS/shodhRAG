import React from "react";
import { Composition } from "remotion";
import "./fonts";
import { LOOP_SHOTS, slots, type Slot } from "./cut";
import { Act1, type Palette } from "./act1";
import { barBeat, beatFrame } from "./beats";
import { LaunchVideo } from "./launch";
import { FPS } from "./theme";

const launch = slots();

/** The loop's shots back to back, each at its length in the launch cut. */
const loop: Slot[] = LOOP_SHOTS.map((id) => {
  const slot = launch.find((s) => s.id === id);
  if (!slot) throw new Error(`The launch cut has no shot "${id}"`);
  return slot;
}).reduce<Slot[]>((list, slot) => {
  const start = list.length === 0 ? 0 : list[list.length - 1].start + list[list.length - 1].frames;
  return [...list, { ...slot, start }];
}, []);

const total = (list: Slot[]) => list[list.length - 1].start + list[list.length - 1].frames;

/** Act 1 runs eight bars, plus a beat of hold on the page. */
const ACT1_FRAMES = beatFrame(barBeat(8)) + 30;
const PALETTES: Palette[] = ["night", "paper", "ember"];

export const Root: React.FC = () => (
  <>
    {PALETTES.map((palette) => (
      <Composition
        key={palette}
        id={`Act1-${palette}`}
        component={Act1}
        durationInFrames={ACT1_FRAMES}
        fps={FPS}
        width={1920}
        height={1080}
        defaultProps={{ palette }}
      />
    ))}
    <Composition
      id="ShodhLaunch"
      component={LaunchVideo}
      durationInFrames={total(launch)}
      fps={FPS}
      width={1920}
      height={1080}
      defaultProps={{ list: launch, music: true }}
    />
    <Composition
      id="ShodhLoop"
      component={LaunchVideo}
      durationInFrames={total(loop)}
      fps={FPS}
      width={1920}
      height={1080}
      defaultProps={{ list: loop, music: false }}
    />
  </>
);
