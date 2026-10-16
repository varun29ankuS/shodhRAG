import React from "react";
import {
  AbsoluteFill,
  Html5Audio,
  Img,
  OffthreadVideo,
  Sequence,
  interpolate,
  spring,
  staticFile,
  useCurrentFrame,
  useVideoConfig,
} from "remotion";
import { beatFrame, pulse } from "./beats";
import type { Slot } from "./cut";
import { aspectOf, cameraAt, footageFor, mediaOf, type Media } from "./footage";
import { color, font, snappy, whipOut } from "./theme";

const clamp = { extrapolateLeft: "clamp", extrapolateRight: "clamp" } as const;

/** Frames a shot takes to whip in, and to whip out at its end. */
const WHIP_IN = 9;
const WHIP_OUT = 6;
const MUSIC_LEVEL = 0.85;

/** Shodh-orange ground that brightens on every beat (harder on bar starts). */
const Ground: React.FC = () => {
  const frame = useCurrentFrame();
  const hit = pulse(frame, 12);
  return (
    <AbsoluteFill
      style={{
        background: `radial-gradient(120% 95% at 50% 42%, ${color.groundHi} 0%, ${color.accent} 52%, ${color.groundLo} 100%)`,
      }}
    >
      <AbsoluteFill
        style={{
          background: "radial-gradient(60% 55% at 50% 45%, rgba(255,255,255,0.55) 0%, rgba(255,255,255,0) 70%)",
          opacity: 0.22 * hit,
        }}
      />
    </AbsoluteFill>
  );
};

/** Where the footage window sits: wide, under a band for the caption. */
function windowBox(width: number, height: number) {
  const w = width * 0.8;
  const h = (w * 9) / 16;
  const top = height - h - height * 0.045;
  return { left: (width - w) / 2, top, w, h };
}

/** A caption whose words pop in one after another as the shot lands. */
const Caption: React.FC<{ text: string; top: number; size: number; exit: number }> = ({ text, top, size, exit }) => {
  const frame = useCurrentFrame();
  const { fps, width } = useVideoConfig();
  const words = text.split(" ");
  return (
    <div
      style={{
        position: "absolute",
        left: 0,
        right: 0,
        top,
        display: "flex",
        justifyContent: "center",
        flexWrap: "wrap",
        gap: `0 ${size * 0.26}px`,
        maxWidth: width * 0.86,
        margin: "0 auto",
        fontFamily: font.sans,
        fontWeight: 720,
        fontSize: size,
        lineHeight: 1.08,
        letterSpacing: "-0.025em",
        color: color.onAccent,
        textShadow: "0 3px 18px rgba(80, 20, 0, 0.35)",
        opacity: 1 - exit,
      }}
    >
      {words.map((word, i) => {
        const pop = spring({ frame: frame - 3 - i * 3, fps, config: { damping: 13, stiffness: 210, mass: 0.6 } });
        return (
          <span
            key={`${word}-${i}`}
            style={{
              display: "inline-block",
              opacity: interpolate(pop, [0, 0.35], [0, 1], clamp),
              transform: `translateY(${(1 - pop) * size * 0.55}px) scale(${0.7 + 0.3 * pop})`,
            }}
          >
            {word}
          </span>
        );
      })}
    </div>
  );
};

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
      const index = Math.min(media.srcs.length - 1, Math.floor((frame / durationInFrames) * media.srcs.length));
      return <Img src={staticFile(media.srcs[index])} style={fill} />;
    }
  }
};

/**
 * One shot of real app footage: the window whips in from the right, the
 * camera punches in on the manifest's focus, the window kicks on each beat,
 * and it whips out to the left as the next shot comes in.
 */
const FootageShot: React.FC<{ slot: Slot; first: boolean }> = ({ slot, first }) => {
  const frame = useCurrentFrame();
  const { width, height, durationInFrames } = useVideoConfig();
  const entry = footageFor(slot.id);
  const media = mediaOf(slot.id, entry);
  const camera = cameraAt(entry.keyframes, frame / Math.max(1, durationInFrames - 1));
  const aspect = aspectOf(entry);
  const box = windowBox(width, height);

  const enter = first ? 1 : interpolate(frame, [0, WHIP_IN], [0, 1], { ...clamp, easing: snappy });
  const leave = interpolate(frame, [durationInFrames - WHIP_OUT, durationInFrames], [0, 1], { ...clamp, easing: whipOut });
  const x = (1 - enter) * width * 0.75 - leave * width * 0.85;
  const blur = (1 - enter) * 14 + leave * 18;
  const kick = 1 + 0.014 * pulse(slot.start + frame, 8);
  const scale = (0.9 + 0.1 * enter) * kick;
  const tilt = (1 - enter) * -4 + leave * 3;

  // Cover-fit the footage into the window, centred on the camera's focus.
  const boxAspect = box.w / box.h;
  const mediaW = boxAspect < aspect ? box.h * aspect : box.w;
  const mediaH = boxAspect < aspect ? box.h : box.w / aspect;
  const fit = (v: number, min: number) => Math.min(0, Math.max(min, v));
  const left = fit(box.w / 2 - camera.cx * mediaW, box.w - mediaW);
  const top = fit(box.h / 2 - camera.cy * mediaH, box.h - mediaH);

  return (
    <AbsoluteFill>
      <Caption text={slot.caption} top={height * 0.035} size={height * 0.068} exit={leave} />
      <div
        style={{
          position: "absolute",
          left: box.left,
          top: box.top,
          width: box.w,
          height: box.h,
          borderRadius: 16,
          overflow: "hidden",
          background: color.surface,
          boxShadow: "0 40px 90px rgba(70, 18, 0, 0.45), 0 8px 24px rgba(70, 18, 0, 0.30)",
          transform: `translateX(${x}px) scale(${scale}) rotate(${tilt}deg)`,
          filter: blur > 0.3 ? `blur(${blur}px)` : undefined,
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
    </AbsoluteFill>
  );
};

/** Lands on a beat (frames relative to the close's start): 0 → 1 with overshoot. */
function useSlam(at: number): number {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  return spring({ frame: frame - at, fps, config: { damping: 11, stiffness: 240, mass: 0.7 } });
}

/** The close: logo, name, line, and where to get it, each slammed in on a bar start. */
const Close: React.FC<{ slot: Slot }> = ({ slot }) => {
  const { height } = useVideoConfig();
  const bar = (n: number) => beatFrame(slot.from + 4 * n) - slot.start;
  const logo = useSlam(0);
  const name = useSlam(bar(1));
  const line = useSlam(bar(2));
  const get = useSlam(bar(3));
  const tile = height * 0.2;
  const text: React.CSSProperties = { fontFamily: font.sans, color: color.onAccent, textAlign: "center" };
  const slam = (v: number, from = 1.6): React.CSSProperties => ({
    opacity: interpolate(v, [0, 0.3], [0, 1], clamp),
    transform: `scale(${from + (1 - from) * v})`,
  });
  return (
    <AbsoluteFill style={{ alignItems: "center", justifyContent: "center", flexDirection: "column", gap: height * 0.03 }}>
      <div
        style={{
          width: tile,
          height: tile,
          borderRadius: tile * 0.24,
          background: color.surface,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          boxShadow: "0 30px 70px rgba(70, 18, 0, 0.45)",
          ...slam(logo, 0.4),
        }}
      >
        <Img src={staticFile("shodh-logo.svg")} style={{ width: tile * 0.78, height: tile * 0.78 }} />
      </div>
      <div style={{ ...text, fontWeight: 760, fontSize: height * 0.13, letterSpacing: "-0.04em", lineHeight: 1, ...slam(name) }}>Shodh</div>
      <div style={{ ...text, fontWeight: 600, fontSize: height * 0.055, letterSpacing: "-0.02em", ...slam(line, 1.3) }}>{slot.caption}</div>
      <div
        style={{
          ...text,
          fontWeight: 560,
          fontSize: height * 0.032,
          padding: `${height * 0.012}px ${height * 0.03}px`,
          borderRadius: 999,
          background: "rgba(255,255,255,0.16)",
          border: "2px solid rgba(255,255,255,0.55)",
          ...slam(get, 1.3),
        }}
      >
        Free and open source · Windows and Mac · github.com/varun29ankuS/shodhRAG
      </div>
    </AbsoluteFill>
  );
};

export const LaunchVideo: React.FC<{ list: Slot[]; music: boolean }> = ({ list, music }) => {
  const { durationInFrames, fps } = useVideoConfig();
  return (
    <AbsoluteFill style={{ background: color.accent }}>
      <Ground />
      {list.map((slot, i) => (
        <Sequence key={slot.id} from={slot.start} durationInFrames={slot.frames} name={slot.id}>
          {slot.id === "close" ? <Close slot={slot} /> : <FootageShot slot={slot} first={i === 0} />}
        </Sequence>
      ))}
      {music ? (
        <Html5Audio
          src={staticFile("music/track.mp3")}
          volume={(f) =>
            MUSIC_LEVEL * interpolate(f, [durationInFrames - Math.round(1.9 * fps), durationInFrames], [1, 0], clamp)
          }
        />
      ) : null}
    </AbsoluteFill>
  );
};
