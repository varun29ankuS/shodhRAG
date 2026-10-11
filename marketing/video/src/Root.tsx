import React from "react";
import { Composition } from "remotion";
import "./fonts";
import { ShodhVideo } from "./Video";
import { FPS } from "./theme";
import { LAUNCH, LOOP, LOOP_SECONDS, VERTICAL, slots, totalFrames } from "./timeline";

const launch = slots(LAUNCH);
const vertical = slots(VERTICAL);
const loop = slots(LOOP, LOOP_SECONDS);

export const Root: React.FC = () => (
  <>
    <Composition
      id="ShodhLaunch"
      component={ShodhVideo}
      durationInFrames={totalFrames(launch)}
      fps={FPS}
      width={1920}
      height={1080}
      defaultProps={{ list: launch, music: true }}
    />
    <Composition
      id="ShodhLaunchVertical"
      component={ShodhVideo}
      durationInFrames={totalFrames(vertical)}
      fps={FPS}
      width={1080}
      height={1920}
      defaultProps={{ list: vertical, music: true }}
    />
    <Composition
      id="ShodhLoop"
      component={ShodhVideo}
      durationInFrames={totalFrames(loop)}
      fps={FPS}
      width={1920}
      height={1080}
      defaultProps={{ list: loop, music: false }}
    />
  </>
);
