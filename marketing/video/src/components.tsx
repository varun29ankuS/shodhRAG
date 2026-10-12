import React from "react";
import {
  AbsoluteFill,
  Img,
  OffthreadVideo,
  interpolate,
  staticFile,
  useCurrentFrame,
  useVideoConfig,
} from "remotion";
import { aspectOf, cameraAt, footageFor, mediaOf, type FootageEntry, type Media } from "./footage";
import { calm, color, font } from "./theme";

const FADE = 15;

/** Opacity for an element shown inside a sequence: eased in and out. */
export function useFade(fadeFrames = FADE, delay = 0): number {
  const frame = useCurrentFrame();
  const { durationInFrames } = useVideoConfig();
  return interpolate(
    frame,
    [delay, delay + fadeFrames, durationInFrames - fadeFrames, durationInFrames],
    [0, 1, 1, 0],
    { extrapolateLeft: "clamp", extrapolateRight: "clamp", easing: calm },
  );
}

export const isPortrait = (width: number, height: number) => height > width;

/** Bottom-centre caption in the app font. */
export const Caption: React.FC<{ text: string; delay?: number }> = ({ text, delay = 8 }) => {
  const { width, height } = useVideoConfig();
  const opacity = useFade(18, delay);
  const frame = useCurrentFrame();
  const rise = interpolate(frame, [delay, delay + 26], [10, 0], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: calm,
  });
  const portrait = isPortrait(width, height);
  return (
    <AbsoluteFill
      style={{
        justifyContent: "flex-end",
        alignItems: "center",
        paddingBottom: portrait ? height * 0.07 : height * 0.055,
      }}
    >
      <div
        style={{
          maxWidth: portrait ? width * 0.84 : width * 0.66,
          textAlign: "center",
          textWrap: "balance",
          fontFamily: font.sans,
          fontWeight: 450,
          fontSize: portrait ? 46 : 36,
          lineHeight: 1.35,
          letterSpacing: "-0.01em",
          color: color.light.text,
          opacity,
          transform: `translateY(${rise}px)`,
        }}
      >
        {text}
      </div>
    </AbsoluteFill>
  );
};

export const Logo: React.FC<{ size: number }> = ({ size }) => (
  <Img src={staticFile("shodh-logo.svg")} style={{ width: size, height: size }} />
);

/** Where the footage sits in the frame (caption space kept below it). */
export function footageBox(width: number, height: number): { left: number; top: number; w: number; h: number } {
  if (isPortrait(width, height)) {
    const w = width * 0.9;
    const h = height * 0.66;
    return { left: (width - w) / 2, top: height * 0.07, w, h };
  }
  const w = width * 0.75;
  const h = (w * 9) / 16;
  return { left: (width - w) / 2, top: height * 0.065, w, h };
}

/**
 * One scene's footage in a soft window frame, with the manifest's camera
 * moves. In a frame narrower than the footage (vertical cut) it is
 * cover-fitted around the camera's focus, so the reframe follows it.
 */
export const FootageFrame: React.FC<{
  scene: number;
  box: { left: number; top: number; w: number; h: number };
}> = ({ scene, box }) => {
  const frame = useCurrentFrame();
  const { durationInFrames } = useVideoConfig();
  const entry = footageFor(scene);
  const media = mediaOf(scene, entry);
  const camera = cameraAt(entry.keyframes, frame / Math.max(1, durationInFrames - 1));
  const aspect = aspectOf(entry);

  // Cover-fit the footage into the box.
  const boxAspect = box.w / box.h;
  const mediaW = boxAspect < aspect ? box.h * aspect : box.w;
  const mediaH = boxAspect < aspect ? box.h : box.w / aspect;
  const clamp = (v: number, min: number) => Math.min(0, Math.max(min, v));
  const left = clamp(box.w / 2 - camera.cx * mediaW, box.w - mediaW);
  const top = clamp(box.h / 2 - camera.cy * mediaH, box.h - mediaH);
  const opacity = useFootageFade(entry.fade);

  return (
    <div
      style={{
        position: "absolute",
        left: box.left,
        top: box.top,
        width: box.w,
        height: box.h,
        borderRadius: 18,
        overflow: "hidden",
        background: color.light.surface,
        border: `1px solid ${color.light.border}`,
        boxShadow: "0 30px 80px rgba(24, 24, 27, 0.10), 0 4px 14px rgba(24, 24, 27, 0.05)",
        opacity,
      }}
    >
      <div
        style={{
          position: "absolute",
          left,
          top,
          width: mediaW,
          height: mediaH,
          transformOrigin: `${camera.cx * 100}% ${camera.cy * 100}%`,
          transform: `scale(${camera.scale})`,
        }}
      >
        <MediaView media={media} />
      </div>
    </div>
  );
};

/**
 * Opacity of a footage window: eased in and out over 18 frames, except at an
 * edge the manifest marks as continuing (a take that runs on into the next
 * scene, or the very first frame, which must already show the app).
 */
function useFootageFade(fade: FootageEntry["fade"]): number {
  const frame = useCurrentFrame();
  const { durationInFrames } = useVideoConfig();
  const edge = 18;
  const fadeIn = fade?.in ?? true;
  const fadeOut = fade?.out ?? true;
  const opening = fadeIn ? interpolate(frame, [0, edge], [0, 1], { extrapolateLeft: "clamp", extrapolateRight: "clamp", easing: calm }) : 1;
  const closing = fadeOut
    ? interpolate(frame, [durationInFrames - edge, durationInFrames], [1, 0], { extrapolateLeft: "clamp", extrapolateRight: "clamp", easing: calm })
    : 1;
  return Math.min(opening, closing);
}

const fill: React.CSSProperties = { width: "100%", height: "100%", objectFit: "cover" };

const MediaView: React.FC<{ media: Media }> = ({ media }) => {
  const frame = useCurrentFrame();
  const { durationInFrames, fps } = useVideoConfig();
  switch (media.kind) {
    case "video":
      return <OffthreadVideo src={staticFile(media.src)} muted style={fill} />;
    case "frames": {
      const index = Math.min(media.srcs.length - 1, Math.floor((frame * media.fps) / fps));
      return <Img src={staticFile(media.srcs[index])} style={fill} />;
    }
    case "stills": {
      const each = durationInFrames / media.srcs.length;
      return (
        <AbsoluteFill>
          {media.srcs.map((src, i) => {
            const start = i * each;
            const opacity =
              i === 0
                ? 1
                : interpolate(frame, [start - 8, start + 8], [0, 1], {
                    extrapolateLeft: "clamp",
                    extrapolateRight: "clamp",
                  });
            return (
              <AbsoluteFill key={src} style={{ opacity }}>
                <Img src={staticFile(src)} style={fill} />
              </AbsoluteFill>
            );
          })}
        </AbsoluteFill>
      );
    }
  }
};
