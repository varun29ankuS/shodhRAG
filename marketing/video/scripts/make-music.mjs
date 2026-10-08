// Generates the soundtrack: an original, calm ambient pad (no third-party
// audio). Slow progression of open-voiced chords with soft attack and release,
// a quiet root an octave down, gentle stereo detune and a low-pass, faded in
// and out. Deterministic: the same file every run.
//
// Output: public/music/shodh-ambient.wav (16-bit PCM stereo, 44.1 kHz, 75 s).

import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const SAMPLE_RATE = 44100;
const DURATION_S = 75;
const FADE_IN_S = 3;
const FADE_OUT_S = 6;
const PEAK = 10 ** (-14 / 20); // -14 dBFS: the video mixes it lower still.

const OUT = join(dirname(fileURLToPath(import.meta.url)), "..", "public", "music", "shodh-ambient.wav");

const midiToHz = (note) => 440 * 2 ** ((note - 69) / 12);

// D major colour: Dmaj9, Bm11, Gmaj9 (#11 omitted), A6sus2. Played twice.
const PROGRESSION = [
  { root: 38, notes: [62, 66, 69, 73, 76] },
  { root: 35, notes: [62, 66, 69, 71, 76] },
  { root: 31, notes: [62, 66, 69, 71, 74] },
  { root: 33, notes: [61, 64, 66, 69, 71] },
];
const CHORD_COUNT = PROGRESSION.length * 2;
const CHORD_S = DURATION_S / CHORD_COUNT;
const ATTACK_S = 2.8;
const RELEASE_S = 3.2;

// Soft harmonic recipe: mostly fundamental, a little warmth above it.
const HARMONICS = [
  [1, 1],
  [2, 0.28],
  [3, 0.09],
];

function envelope(t, start, end) {
  if (t < start || t > end + RELEASE_S) return 0;
  const rise = Math.min(1, (t - start) / ATTACK_S);
  const fall = t > end ? Math.max(0, 1 - (t - end) / RELEASE_S) : 1;
  // Raised-cosine edges: no clicks, no audible corners.
  const smooth = (x) => 0.5 - 0.5 * Math.cos(Math.PI * x);
  return smooth(rise) * smooth(fall);
}

function render() {
  const frames = SAMPLE_RATE * DURATION_S;
  const left = new Float64Array(frames);
  const right = new Float64Array(frames);

  for (let c = 0; c < CHORD_COUNT; c++) {
    const chord = PROGRESSION[c % PROGRESSION.length];
    const start = c * CHORD_S - (c === 0 ? 0 : 1.2);
    const end = (c + 1) * CHORD_S;
    const voices = [
      ...chord.notes.map((n) => ({ hz: midiToHz(n), gain: 0.16 })),
      { hz: midiToHz(chord.root), gain: 0.22 },
    ];
    const first = Math.max(0, Math.floor(start * SAMPLE_RATE));
    const last = Math.min(frames, Math.ceil((end + RELEASE_S) * SAMPLE_RATE));
    voices.forEach((voice, v) => {
      const pan = 0.5 + 0.35 * Math.sin(v * 1.7);
      const detune = 1 + 0.0016 * (v % 2 === 0 ? 1 : -1);
      const phase = v * 0.9;
      for (let i = first; i < last; i++) {
        const t = i / SAMPLE_RATE;
        const env = envelope(t, start, end);
        if (env === 0) continue;
        const swell = 0.85 + 0.15 * Math.sin(2 * Math.PI * 0.07 * t + phase);
        let l = 0;
        let r = 0;
        for (const [h, a] of HARMONICS) {
          l += a * Math.sin(2 * Math.PI * voice.hz * h * t + phase);
          r += a * Math.sin(2 * Math.PI * voice.hz * h * detune * t + phase * 1.3);
        }
        const g = voice.gain * env * swell;
        left[i] += g * l * (1 - pan) * 2;
        right[i] += g * r * pan * 2;
      }
    });
  }

  // One-pole low-pass (~1.6 kHz) for a soft, distant tone.
  const alpha = 1 - Math.exp((-2 * Math.PI * 1600) / SAMPLE_RATE);
  for (const ch of [left, right]) {
    let y = 0;
    for (let i = 0; i < frames; i++) {
      y += alpha * (ch[i] - y);
      ch[i] = y;
    }
  }

  let peak = 0;
  for (let i = 0; i < frames; i++) {
    peak = Math.max(peak, Math.abs(left[i]), Math.abs(right[i]));
  }
  const norm = peak > 0 ? PEAK / peak : 0;

  const pcm = Buffer.alloc(frames * 4);
  for (let i = 0; i < frames; i++) {
    const t = i / SAMPLE_RATE;
    const fade = Math.min(1, t / FADE_IN_S, (DURATION_S - t) / FADE_OUT_S);
    const g = norm * Math.max(0, fade);
    pcm.writeInt16LE(Math.round(Math.max(-1, Math.min(1, left[i] * g)) * 32767), i * 4);
    pcm.writeInt16LE(Math.round(Math.max(-1, Math.min(1, right[i] * g)) * 32767), i * 4 + 2);
  }
  return pcm;
}

function wav(pcm) {
  const header = Buffer.alloc(44);
  header.write("RIFF", 0);
  header.writeUInt32LE(36 + pcm.length, 4);
  header.write("WAVE", 8);
  header.write("fmt ", 12);
  header.writeUInt32LE(16, 16);
  header.writeUInt16LE(1, 20); // PCM
  header.writeUInt16LE(2, 22); // stereo
  header.writeUInt32LE(SAMPLE_RATE, 24);
  header.writeUInt32LE(SAMPLE_RATE * 4, 28);
  header.writeUInt16LE(4, 32);
  header.writeUInt16LE(16, 34);
  header.write("data", 36);
  header.writeUInt32LE(pcm.length, 40);
  return Buffer.concat([header, pcm]);
}

mkdirSync(dirname(OUT), { recursive: true });
writeFileSync(OUT, wav(render()));
console.log(`Wrote ${OUT}`);
