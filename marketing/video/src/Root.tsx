import React from "react";
import { Composition } from "remotion";
import "./fonts";
import { LOOP_SHOTS, slots, type Slot } from "./cut";
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

export const Root: React.FC = () => (
  <>
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
