/**
 * The phone's UI rules, read from its stylesheet and sources:
 * - text is 12px or larger;
 * - only `transform` and `opacity` are animated (CSS transitions, keyframes,
 *   and every motion value in the code), never a layout property;
 * - a citation chip, drawn small on the line, has a 44px area to tap.
 * (The 44px size of every other control is checked on the rendered screens,
 * tests/screens.test.tsx.)
 */
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { PRESETS } from '../src/lib/motion';

const css = readFileSync(new URL('../src/theme.css', import.meta.url), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');

function sources(dir: string): Array<{ file: string; text: string }> {
  const out: Array<{ file: string; text: string }> = [];
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) out.push(...sources(path));
    else if (/\.(ts|tsx)$/.test(name)) out.push({ file: path, text: readFileSync(path, 'utf8') });
  }
  return out;
}
const code = sources(new URL('../src', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1'));

const ANIMATABLE = new Set(['transform', 'opacity']);
/** Motion values that are transforms or opacity, and the keys that only configure them. */
const MOTION_KEYS = new Set(['x', 'y', 'scale', 'scaleX', 'scaleY', 'rotate', 'opacity', 'transition', 'transitionEnd']);

describe('text', () => {
  it('is 12px or larger in the stylesheet', () => {
    const sizes = [...css.matchAll(/font-size:\s*([^;]+);/g)].map(m => m[1].trim());
    expect(sizes.length).toBeGreaterThan(10);
    for (const size of sizes) {
      const px = /^(\d+(?:\.\d+)?)px$/.exec(size);
      expect(px, `font-size ${size} is written in px`).not.toBeNull();
      expect(Number(px?.[1]), `font-size ${size}`).toBeGreaterThanOrEqual(12);
    }
  });

  it('is 12px or larger in the screens', () => {
    for (const { file, text } of code) {
      for (const match of text.matchAll(/fontSize:\s*([^,}\s]+)/g)) {
        expect(Number(match[1]), `${file}: fontSize ${match[1]}`).toBeGreaterThanOrEqual(12);
      }
    }
  });
});

describe('motion', () => {
  it('in the stylesheet, transitions and keyframes move only transform and opacity', () => {
    const transitions = [...css.matchAll(/transition(?:-property)?:\s*([^;]+);/g)].map(m => m[1].trim());
    expect(transitions.length).toBeGreaterThan(0);
    for (const transition of transitions) {
      if (transition === 'none') continue;
      // Commas inside parentheses (a cubic-bezier) do not separate transitions.
      for (const part of transition.split(/,(?![^(]*\))/)) {
        const property = part.trim().split(/\s+/)[0];
        expect(ANIMATABLE.has(property), `transition of "${property}" (${transition})`).toBe(true);
      }
    }
    const keyframes = [...css.matchAll(/@keyframes\s+([\w-]+)\s*\{((?:[^{}]*\{[^{}]*\})*)\s*\}/g)];
    expect(keyframes.length).toBeGreaterThanOrEqual(3);
    for (const [, name, body] of keyframes) {
      for (const declaration of body.matchAll(/([\w-]+)\s*:/g)) {
        expect(ANIMATABLE.has(declaration[1]), `@keyframes ${name} animates ${declaration[1]}`).toBe(true);
      }
    }
    // Every animation names one of those keyframes.
    const names = new Set(keyframes.map(k => k[1]));
    for (const animation of css.matchAll(/animation:\s*([^;]+);/g)) {
      if (animation[1].trim() === 'none') continue;
      const used = animation[1].trim().split(/\s+/)[0];
      expect(names.has(used), `animation ${animation[1]}`).toBe(true);
    }
  });

  it('in the code, every animated value is a transform or opacity', () => {
    for (const { file, text } of code) {
      // No CSS animation or transition written inline, and no layout animation.
      expect(text, `${file}: an inline CSS transition`).not.toMatch(/\btransition:\s*['"`]/);
      expect(text, `${file}: an inline CSS animation`).not.toMatch(/\banimation:\s*['"`]/);
      expect(text, `${file}: a layout animation`).not.toMatch(/\slayout(Id)?(=|\s|>)/);
      for (const match of text.matchAll(/\b(initial|animate|exit|while\w+)=\{\{([^{}]*)\}\}/g)) {
        for (const key of match[2].matchAll(/(\w+)\s*:/g)) {
          expect(MOTION_KEYS.has(key[1]), `${file}: ${match[1]} animates ${key[1]}`).toBe(true);
        }
      }
      // Anything more is a preset of lib/motion.ts, checked below.
      expect(text, `${file}: a nested motion value`).not.toMatch(/\b(initial|animate|exit|while\w+)=\{\{[^}]*\{/);
    }
  });

  it('the presets move only transforms and opacity', () => {
    const check = (where: string, value: unknown) => {
      if (typeof value === 'function') value = (value as (i: number) => unknown)(1);
      for (const key of Object.keys(value as object)) expect(MOTION_KEYS.has(key), `${where} animates ${key}`).toBe(true);
    };
    for (const [name, variants] of Object.entries(PRESETS)) {
      for (const [state, value] of Object.entries(variants)) check(`${name}.${state}`, value);
    }
  });

  it('with "Remove animations" on, words fade in and nothing else moves', () => {
    const reduced = /@media \(prefers-reduced-motion: reduce\) \{([\s\S]*)\}\s*$/.exec(css.trim())?.[1] ?? '';
    expect(reduced).toMatch(/\.word\s*\{\s*animation: fade-in/);
    expect(reduced).toMatch(/\.skeleton::after[\s\S]*animation: none/);
    expect(reduced).toMatch(/transition: none/);
    const app = code.find(c => c.file.endsWith('App.tsx'))?.text ?? '';
    expect(app).toContain('<MotionConfig reducedMotion="user">');
  });
});

describe('a citation chip', () => {
  it('is small on the line and 44px to the finger', () => {
    const chip = /\.chip\s*\{([^}]*)\}/.exec(css)?.[1] ?? '';
    const before = /\.chip::before\s*\{([^}]*)\}/.exec(css)?.[1] ?? '';
    const size = Number(/\bwidth:\s*(\d+)px/.exec(chip)?.[1]);
    const height = Number(/\bheight:\s*(\d+)px/.exec(chip)?.[1]);
    const inset = Math.abs(Number(/inset:\s*(-?\d+)px/.exec(before)?.[1]));
    expect(size + 2 * inset).toBeGreaterThanOrEqual(44);
    expect(height + 2 * inset).toBeGreaterThanOrEqual(44);
    expect(chip).toMatch(/position:\s*relative/);
    expect(before).toMatch(/position:\s*absolute/);
  });
});
