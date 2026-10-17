import React from "react";
import {
  AbsoluteFill,
  Easing,
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
import { barBeat, beatFrame } from "./beats";
import { FOOTAGE_SHOTS, type Slot } from "./cut";
import { aspectOf, cameraAt, footageFor, mediaOf, type Media } from "./footage";
import { STAGE, Vignette } from "./vignettes";
import { color, font } from "./theme";

const clamp = { extrapolateLeft: "clamp", extrapolateRight: "clamp" } as const;
const expoOut = Easing.bezier(0.16, 1, 0.3, 1);
const expoIn = Easing.bezier(0.7, 0, 0.84, 0);
const MUSIC_LEVEL = 1;

/** The layout grid at 1920×1080: a text column on the left, the app on the right. */
const GRID = {
  margin: 120,
  textWidth: 580,
  windowLeft: 760,
  windowWidth: 1040,
} as const;

/** Frames the left column and the window take to leave at the end of a shot. */
const EXIT = 9;

/**
 * Shodh orange, lit: a diagonal gradient, two slow drifting lights, a
 * vignette and a fine grain. Nothing on it moves to the beat.
 */
const Ground: React.FC = () => {
  const frame = useCurrentFrame();
  const { width, height } = useVideoConfig();
  const t = frame / 30;
  const a = { x: 0.28 + 0.06 * Math.sin(t * 0.35), y: 0.3 + 0.05 * Math.cos(t * 0.28) };
  const b = { x: 0.78 + 0.05 * Math.cos(t * 0.3), y: 0.72 + 0.06 * Math.sin(t * 0.33) };
  return (
    <AbsoluteFill style={{ background: `linear-gradient(135deg, ${color.groundHi} 0%, ${color.accent} 48%, ${color.groundLo} 100%)` }}>
      <AbsoluteFill
        style={{
          background: `radial-gradient(42% 52% at ${a.x * 100}% ${a.y * 100}%, rgba(255,214,170,0.42) 0%, rgba(255,214,170,0) 70%)`,
        }}
      />
      <AbsoluteFill
        style={{
          background: `radial-gradient(38% 48% at ${b.x * 100}% ${b.y * 100}%, rgba(255,150,90,0.30) 0%, rgba(255,150,90,0) 70%)`,
        }}
      />
      <AbsoluteFill style={{ background: "radial-gradient(120% 90% at 50% 45%, rgba(0,0,0,0) 55%, rgba(60,14,0,0.42) 100%)" }} />
      <svg width={width} height={height} style={{ position: "absolute", inset: 0, mixBlendMode: "overlay", opacity: 0.14 }}>
        <filter id="grain">
          <feTurbulence type="fractalNoise" baseFrequency="0.85" numOctaves={2} seed={Math.floor(frame / 2) % 8} stitchTiles="stitch" />
          <feColorMatrix type="saturate" values="0" />
        </filter>
        <rect width="100%" height="100%" filter="url(#grain)" />
      </svg>
    </AbsoluteFill>
  );
};

/** A line of text that rises out of a mask. */
const RevealLine: React.FC<{ children: React.ReactNode; at: number; exitAt: number; style: React.CSSProperties }> = ({
  children,
  at,
  exitAt,
  style,
}) => {
  const frame = useCurrentFrame();
  const enter = interpolate(frame, [at, at + 18], [1, 0], { ...clamp, easing: expoOut });
  const leave = interpolate(frame, [exitAt, exitAt + EXIT], [0, 1], { ...clamp, easing: expoIn });
  return (
    <div style={{ overflow: "hidden", paddingBottom: "0.08em", marginBottom: "-0.08em" }}>
      <div style={{ ...style, transform: `translateY(${(enter * 105 - leave * 105).toFixed(2)}%)` }}>{children}</div>
    </div>
  );
};

/** The left column: the chapter's animated scene, its label, headline and one line of detail. */
const TextColumn: React.FC<{ slot: Slot; index: number }> = ({ slot, index }) => {
  const frame = useCurrentFrame();
  const { fps, durationInFrames } = useVideoConfig();
  const exitAt = durationInFrames - EXIT;
  const tile = spring({ frame, fps, config: { damping: 14, stiffness: 180, mass: 0.7 } });
  const tileOut = interpolate(frame, [exitAt + 2, durationInFrames], [0, 1], { ...clamp, easing: expoIn });
  const detailIn = interpolate(frame, [14, 30], [0, 1], { ...clamp, easing: expoOut });
  const detailOut = interpolate(frame, [exitAt - 2, exitAt + 6], [0, 1], { ...clamp, easing: expoIn });
  const number = String(index + 1).padStart(2, "0");
  return (
    <div
      style={{
        position: "absolute",
        left: GRID.margin,
        top: 0,
        bottom: 0,
        width: GRID.textWidth,
        display: "flex",
        flexDirection: "column",
        justifyContent: "center",
        fontFamily: font.sans,
        color: color.onAccent,
      }}
    >
      <div
        style={{
          width: STAGE.width + 40,
          height: STAGE.height + 32,
          boxSizing: "border-box",
          padding: "16px 20px",
          borderRadius: 28,
          background: "rgba(255,255,255,0.10)",
          border: "1.5px solid rgba(255,255,255,0.30)",
          boxShadow: "inset 0 1px 0 rgba(255,255,255,0.22), 0 24px 50px rgba(70,18,0,0.22)",
          marginBottom: 40,
          opacity: Math.min(1, tile * 1.4) * (1 - tileOut),
          transform: `translateY(${(1 - tile) * 24 - tileOut * 20}px) scale(${0.96 + 0.04 * tile})`,
          transformOrigin: "left center",
        }}
      >
        {slot.id === "close" ? null : <Vignette id={slot.id} frame={frame} fps={fps} />}
      </div>
      <RevealLine
        at={2}
        exitAt={exitAt}
        style={{ fontSize: 20, fontWeight: 650, letterSpacing: "0.24em", textTransform: "uppercase", opacity: 0.82, marginBottom: 22 }}
      >
        <span style={{ fontVariantNumeric: "tabular-nums" }}>{number}</span>
        <span style={{ display: "inline-block", width: 44, height: 2, background: "rgba(255,255,255,0.6)", margin: "0 16px", verticalAlign: "middle" }} />
        {slot.label}
      </RevealLine>
      {slot.lines.map((line, i) => (
        <RevealLine
          key={line}
          at={5 + i * 4}
          exitAt={exitAt + i * 2}
          style={{ fontSize: 62, fontWeight: 760, letterSpacing: "-0.04em", lineHeight: 1.04, whiteSpace: "nowrap", textShadow: "0 4px 30px rgba(80,20,0,0.25)" }}
        >
          {line}
        </RevealLine>
      ))}
      <div
        style={{
          marginTop: 30,
          maxWidth: 470,
          fontSize: 27,
          fontWeight: 460,
          lineHeight: 1.42,
          opacity: 0.88 * detailIn * (1 - detailOut),
          transform: `translateY(${(1 - detailIn) * 16 - detailOut * 12}px)`,
        }}
      >
        {slot.detail}
      </div>
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
 * The app, in a window on the right: it swings in on a perspective turn and
 * settles flat and still, and the camera inside moves from the whole app to
 * the action. It leaves to the left as the shot ends.
 */
const AppWindow: React.FC<{ slot: Slot; first: boolean }> = ({ slot, first }) => {
  const frame = useCurrentFrame();
  const { height, durationInFrames, fps } = useVideoConfig();
  const entry = footageFor(slot.id);
  const media = mediaOf(slot.id, entry);
  const camera = cameraAt(entry.keyframes, frame / Math.max(1, durationInFrames - 1));
  const aspect = aspectOf(entry);
  const w = GRID.windowWidth;
  const h = w / aspect;
  const top = (height - h) / 2;

  const enter = first ? 1 : spring({ frame, fps, config: { damping: 18, stiffness: 120, mass: 0.9 } });
  const leave = interpolate(frame, [durationInFrames - EXIT, durationInFrames], [0, 1], { ...clamp, easing: expoIn });
  const turn = (1 - enter) * -22 + leave * 10;
  const x = (1 - enter) * 160 - leave * 120;
  const scale = (0.92 + 0.08 * enter) * (1 - 0.05 * leave);
  const opacity = Math.min(1, enter * 1.6) * (1 - leave);
  const blur = (1 - enter) * 6 + leave * 10;

  return (
    <div style={{ position: "absolute", inset: 0, perspective: 2400 }}>
      <div
        style={{
          position: "absolute",
          left: GRID.windowLeft,
          top,
          width: w,
          height: h,
          borderRadius: 18,
          overflow: "hidden",
          background: color.surface,
          boxShadow: "0 50px 110px rgba(60,14,0,0.45), 0 12px 30px rgba(60,14,0,0.28), inset 0 0 0 1px rgba(255,255,255,0.5)",
          transform: `translate3d(${x}px, 0, 0) rotateY(${turn}deg) scale(${scale})`,
          transformOrigin: "left center",
          opacity,
          filter: blur > 0.3 ? `blur(${blur}px)` : undefined,
        }}
      >
        <div
          style={{
            position: "absolute",
            inset: 0,
            transformOrigin: `${camera.cx * 100}% ${camera.cy * 100}%`,
            transform: `translate(${pan(camera.cx, w, camera.scale)}px, ${pan(camera.cy, h, camera.scale)}px) scale(${camera.scale})`,
          }}
        >
          <MediaView media={media} />
        </div>
      </div>
    </div>
  );
};

/**
 * How far to shift footage scaled by `scale` about `focus` (0..1 of `size`)
 * to bring the focus toward the centre, without uncovering an edge.
 */
function pan(focus: number, size: number, scale: number): number {
  const want = (0.5 - focus) * size;
  const spare = scale - 1;
  return Math.max(-(1 - focus) * size * spare, Math.min(focus * size * spare, want));
}

/**
 * The brand mark, top left, on screen from the first frame: the logo on its
 * tile and the name. It hands over to the close, where the logo lands large.
 */
const BrandMark: React.FC<{ closeAt: number }> = ({ closeAt }) => {
  const frame = useCurrentFrame();
  const out = interpolate(frame, [closeAt - 8, closeAt + 4], [0, 1], { ...clamp, easing: expoIn });
  if (out >= 1) return null;
  return (
    <div
      style={{
        position: "absolute",
        left: GRID.margin,
        top: 48,
        display: "flex",
        alignItems: "center",
        gap: 20,
        fontFamily: font.sans,
        color: color.onAccent,
        opacity: 1 - out,
        transform: `translateY(${-out * 16}px)`,
      }}
    >
      <div
        style={{
          width: 76,
          height: 76,
          borderRadius: 20,
          background: color.surface,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          boxShadow: "0 10px 24px rgba(60,14,0,0.3)",
        }}
      >
        <Img src={staticFile("shodh-logo.svg")} style={{ width: 58, height: 58 }} />
      </div>
      <div style={{ fontSize: 44, fontWeight: 780, letterSpacing: "-0.035em" }}>Shodh</div>
    </div>
  );
};

const FootageShot: React.FC<{ slot: Slot; index: number }> = ({ slot, index }) => (
  <AbsoluteFill>
    <AppWindow slot={slot} first={index === 0} />
    <TextColumn slot={slot} index={index} />
  </AbsoluteFill>
);

/** Thin segments along the bottom of the text column: one per shot, filling as it plays. */
const Progress: React.FC<{ list: Slot[] }> = ({ list }) => {
  const frame = useCurrentFrame();
  const footage = list.filter((s) => s.id !== "close");
  const last = footage[footage.length - 1];
  if (!last || frame >= last.start + last.frames) return null;
  const fadeIn = interpolate(frame, [6, 20], [0, 1], clamp);
  return (
    <div style={{ position: "absolute", left: GRID.margin, bottom: 86, display: "flex", gap: 10, opacity: fadeIn }}>
      {footage.map((s) => {
        const p = interpolate(frame, [s.start, s.start + s.frames], [0, 1], clamp);
        return (
          <div key={s.id} style={{ width: 54, height: 3, borderRadius: 2, background: "rgba(255,255,255,0.25)", overflow: "hidden" }}>
            <div style={{ width: `${p * 100}%`, height: "100%", background: "rgba(255,255,255,0.95)" }} />
          </div>
        );
      })}
    </div>
  );
};

/** The close: the logo lands, then the name, the line and where to get it, one per bar. */
const Close: React.FC<{ slot: Slot }> = ({ slot }) => {
  const frame = useCurrentFrame();
  const { fps, height } = useVideoConfig();
  const bar = (n: number) => beatFrame(barBeat(slot.bar + n)) - slot.start;
  const logo = spring({ frame, fps, config: { damping: 12, stiffness: 200, mass: 0.8 } });
  const pill = spring({ frame: frame - bar(3), fps, config: { damping: 14, stiffness: 200 } });
  const tile = height * 0.18;
  const ring = interpolate(frame, [0, 26], [0, 1], { ...clamp, easing: expoOut });
  return (
    <AbsoluteFill style={{ alignItems: "center", justifyContent: "center", fontFamily: font.sans, color: color.onAccent }}>
      <div style={{ position: "relative", marginBottom: 46 }}>
        <div
          style={{
            position: "absolute",
            left: "50%",
            top: "50%",
            width: tile * (1 + ring * 1.4),
            height: tile * (1 + ring * 1.4),
            borderRadius: "50%",
            border: "2px solid rgba(255,255,255,0.5)",
            transform: "translate(-50%, -50%)",
            opacity: 1 - ring,
          }}
        />
        <div
          style={{
            width: tile,
            height: tile,
            borderRadius: tile * 0.26,
            background: color.surface,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            boxShadow: "0 36px 80px rgba(60,14,0,0.45)",
            transform: `scale(${0.5 + 0.5 * logo}) rotate(${(1 - logo) * -8}deg)`,
            opacity: Math.min(1, logo * 1.5),
          }}
        >
          <Img src={staticFile("shodh-logo.svg")} style={{ width: tile * 0.76, height: tile * 0.76 }} />
        </div>
      </div>
      <RevealLine at={bar(1)} exitAt={10_000} style={{ fontSize: 150, fontWeight: 780, letterSpacing: "-0.05em", lineHeight: 0.95 }}>
        Shodh
      </RevealLine>
      <div style={{ height: 22 }} />
      <RevealLine at={bar(2)} exitAt={10_000} style={{ fontSize: 50, fontWeight: 600, letterSpacing: "-0.02em", opacity: 0.95 }}>
        {slot.lines[0]}
      </RevealLine>
      <div
        style={{
          marginTop: 46,
          display: "flex",
          gap: 18,
          alignItems: "center",
          fontSize: 26,
          fontWeight: 560,
          padding: "14px 30px",
          borderRadius: 999,
          background: "rgba(255,255,255,0.14)",
          border: "1.5px solid rgba(255,255,255,0.5)",
          opacity: Math.min(1, pill * 1.4),
          transform: `translateY(${(1 - pill) * 24}px)`,
        }}
      >
        <span>{slot.detail}</span>
        <span style={{ width: 6, height: 6, borderRadius: 3, background: "rgba(255,255,255,0.7)" }} />
        <span style={{ fontWeight: 700 }}>github.com/varun29ankuS/shodhRAG</span>
      </div>
    </AbsoluteFill>
  );
};

export const LaunchVideo: React.FC<{ list: Slot[]; music: boolean }> = ({ list, music }) => {
  const { durationInFrames, fps } = useVideoConfig();
  return (
    <AbsoluteFill style={{ background: color.accent }}>
      <Ground />
      {list.map((slot) => (
        <Sequence key={slot.id} from={slot.start} durationInFrames={slot.frames} name={slot.id}>
          {slot.id === "close" ? (
            <Close slot={slot} />
          ) : (
            <FootageShot slot={slot} index={FOOTAGE_SHOTS.findIndex((s) => s.id === slot.id)} />
          )}
        </Sequence>
      ))}
      <Progress list={list} />
      <BrandMark closeAt={list.find((s) => s.id === "close")?.start ?? durationInFrames} />
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
