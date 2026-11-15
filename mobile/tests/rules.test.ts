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
import { join, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { PRESETS } from '../src/lib/motion';

const css = readFileSync(resolve(process.cwd(), 'src/theme.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');

function sources(dir: string): Array<{ file: string; text: string }> {
  const out: Array<{ file: string; text: string }> = [];
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) out.push(...sources(path));
    else if (/\.(ts|tsx)$/.test(name)) out.push({ file: path, text: readFileSync(path, 'utf8') });
  }
  return out;
}
const code = sources(resolve(process.cwd(), 'src'));

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

describe('fonts', () => {
  const INTER = "'Inter Variable', system-ui, sans-serif";
  const MONO = "ui-monospace, 'Cascadia Mono', Menlo, monospace";
  const raw = readFileSync(resolve(process.cwd(), 'src/theme.css'), 'utf8');

  it('are Inter for everything, and one monospace stack for code', () => {
    expect(/--font:\s*([^;]+);/.exec(css)?.[1].trim()).toBe(INTER);
    expect(/--mono:\s*([^;]+);/.exec(css)?.[1].trim()).toBe(MONO);
    const families = [...css.matchAll(/font-family:\s*([^;]+);/g)].map(m => m[1].trim());
    expect(families.length).toBeGreaterThan(0);
    for (const family of families) expect(['var(--font)', 'var(--mono)'], `font-family: ${family}`).toContain(family);
    // The `font` shorthand only inherits.
    for (const font of css.matchAll(/(?:^|[;{\s])font:\s*([^;]+);/g)) expect(font[1].trim()).toBe('inherit');
    for (const { file, text } of code) {
      expect(text, `${file}: a font family written in the code`).not.toMatch(/fontFamily\s*:/);
      // A canvas draws in Inter too (the scripted pages of CI's screenshots).
      for (const font of text.matchAll(/\.font\s*=\s*`([^`]*)`/g)) expect(font[1], file).toContain(INTER);
    }
  });

  it('name no other face: no Newsreader, Literata, Geist or serif anywhere in the phone app', () => {
    const pkg = readFileSync(resolve(process.cwd(), 'package.json'), 'utf8');
    const html = readFileSync(resolve(process.cwd(), 'index.html'), 'utf8');
    const OTHERS = /newsreader|literata|geist|georgia|\bserif\b|times new roman/i;
    expect(raw.replace(/sans-serif/g, ''), 'src/theme.css').not.toMatch(OTHERS);
    expect(html, 'index.html').not.toMatch(OTHERS);
    for (const { file, text } of code) expect(text.replace(/sans-serif/g, ''), file).not.toMatch(OTHERS);
    // The only font the app bundles is Inter, and its licence (SIL OFL 1.1) is kept with the app.
    const fonts = Object.keys(JSON.parse(pkg).dependencies).filter(name => name.startsWith('@fontsource'));
    expect(fonts).toEqual(['@fontsource-variable/inter']);
    const licence = readFileSync(resolve(process.cwd(), 'licenses/Inter-OFL-1.1.txt'), 'utf8');
    expect(licence).toContain('SIL OPEN FONT LICENSE Version 1.1');
    expect(licence).toContain('The Inter Project Authors');
    expect(readFileSync(resolve(process.cwd(), '../NOTICE'), 'utf8')).toContain('mobile/licenses/Inter-OFL-1.1.txt');
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
  it('is small on the line and 44px to the finger, centred on it', () => {
    const chip = /\.chip\s*\{([^}]*)\}/.exec(css)?.[1] ?? '';
    const before = /\.chip::before\s*\{([^}]*)\}/.exec(css)?.[1] ?? '';
    expect(Number(/\bheight:\s*(\d+)px/.exec(chip)?.[1])).toBeLessThanOrEqual(18);
    const width = Number(/\bwidth:\s*(\d+)px/.exec(before)?.[1]);
    const height = Number(/\bheight:\s*(\d+)px/.exec(before)?.[1]);
    expect(width).toBeGreaterThanOrEqual(44);
    expect(height).toBeGreaterThanOrEqual(44);
    // Centred: from the chip's middle, back by half its size.
    expect(before).toMatch(/left:\s*50%/);
    expect(before).toMatch(/top:\s*50%/);
    expect(/\bmargin:\s*([^;]+);/.exec(before)?.[1].trim()).toBe(`-${height / 2}px 0 0 -${width / 2}px`);
    expect(chip).toMatch(/position:\s*relative/);
    expect(before).toMatch(/position:\s*absolute/);
  });

  it('sits flush with the word before it and the punctuation after it: "question¹." not "question ¹ ."', () => {
    const chip = /\.chip\s*\{([^}]*)\}/.exec(css)?.[1] ?? '';
    // As wide as its digits: no empty box on either side of the number.
    expect(/\bwidth:\s*([^;]+);/.exec(chip)?.[1].trim()).toBe('auto');
    expect(/\bpadding:\s*([^;]+);/.exec(chip)?.[1].trim()).toBe('0');
    expect(chip).not.toMatch(/min-width/);
    const margin = /\bmargin:\s*([^;]+);/.exec(chip)?.[1].trim().split(/\s+/) ?? [];
    // margin: top right bottom left; nothing on the right, a hair on the left.
    expect(margin[1] ?? margin[0]).toBe('0');
    expect(Number(/^(\d+)px$/.exec(margin[3] ?? margin[1] ?? margin[0])?.[1] ?? 0)).toBeLessThanOrEqual(1);
  });
});

describe('the follow-up composer', () => {
  const rule = (selector: string) => new RegExp(`\\n${selector.replace('.', '\\.')}\\s*\\{([^}]*)\\}`).exec(css)?.[1] ?? '';
  const pixels = (value: string) => [...value.matchAll(/(\d+)px/g)].reduce((sum, m) => sum + Number(m[1]), 0);
  const declared = (body: string, property: string) => pixels(new RegExp(`(?:^|[\\s;])${property}:\\s*([^;]+);`).exec(body)?.[1] ?? '');

  it('sits above the tab bar, and leaves the answer\'s last line and the Code output readable above both', () => {
    const bar = declared(rule('.tabbar'), 'height');
    expect(bar).toBeGreaterThanOrEqual(44);
    const box = rule('.askbox-floating');
    const bottom = declared(box, 'bottom');
    const above = bottom + declared(box, 'min-height');
    // The field clears the tab bar, with a gap between them.
    expect(bottom).toBeGreaterThanOrEqual(bar + 8);
    const room = declared(rule('.scroll-with-composer'), 'padding-bottom');
    expect(room).toBeGreaterThanOrEqual(above + 16);
    // Every other screen's end clears the tab bar.
    expect(declared(rule('.scroll'), 'padding-bottom')).toBeGreaterThanOrEqual(bar + 16);
    const ask = readFileSync(resolve(process.cwd(), 'src/screens/Ask.tsx'), 'utf8');
    expect(ask).toContain('scroll scroll-with-composer');
  });
});

describe('the launcher icon', () => {
  const res = resolve(process.cwd(), 'src-tauri/icons/android');
  const read = (path: string) => readFileSync(join(res, path), 'utf8');

  it('is adaptive: the whole elephant inside the safe zone, white or black behind it, and a monochrome layer', () => {
    for (const name of ['ic_launcher', 'ic_launcher_round']) {
      const icon = read(`mipmap-anydpi-v26/${name}.xml`);
      expect(icon).toContain('@color/shodh_icon_background');
      expect(icon).toContain('@drawable/shodh_icon_foreground');
      expect(icon).toContain('<monochrome android:drawable="@drawable/shodh_icon_monochrome"');
    }
    const foreground = read('drawable/shodh_icon_foreground.xml');
    // The elephant's layer is scaled into the central 66% of the icon, never full bleed.
    const scale = Number(/android:scaleX="([\d.]+)"/.exec(foreground)?.[1]);
    expect(scale).toBeGreaterThan(0.5);
    expect(scale).toBeLessThanOrEqual(0.7);
    // The elephant itself is unchanged: every shape of the logo is there.
    const svg = readFileSync(resolve(process.cwd(), '../app/public/shodh_logo_nobackground.svg'), 'utf8');
    expect(foreground.match(/<path /g)?.length).toBe(svg.match(/<path\b/g)?.length);
    expect(read('drawable/shodh_icon_monochrome.xml').match(/<path /g)?.length).toBe(svg.match(/<path\b/g)?.length);
    expect(read('values/shodh_icon.xml')).toMatch(/shodh_icon_background">#FFFFFF</);
    expect(read('values-night/shodh_icon.xml')).toMatch(/shodh_icon_background">#000000</);
  });
});
