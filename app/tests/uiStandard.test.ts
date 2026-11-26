/**
 * The standards the interface is held to (docs/superpowers/specs/2026-11-26-ui-international-standard.md),
 * read from the sources: one type scale, one spacing scale, colours from tokens, a visible
 * focus on everything that takes it, targets of 24px, labels that do not wrap, and loops
 * that stop for reduced motion. Each fails on the first file that breaks it. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/uiStandard.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { FOCUS_STYLE, appDir, files, lineOf, openingTags, srcDir, where } from './jsxScan.ts';
import { TYPE_SCALE } from '../src/lib/typeScale.ts';
import { EXTRA_SPACING } from '../src/lib/spacingScale.ts';
import { FOCUS_UNDERLINE } from '../src/lib/focusStyles.ts';

const code = files(srcDir, /\.tsx?$/).map(file => ({ file: where(file), text: readFileSync(file, 'utf8') }));
const views = code.filter(c => c.file.endsWith('.tsx'));
const css = readFileSync(join(srcDir, 'index.css'), 'utf8');
const tailwind = readFileSync(join(appDir, 'tailwind.config.js'), 'utf8');

/** Every match of `pattern` in the sources, as "file: match". */
function found(sources: typeof code, pattern: RegExp): string[] {
  const out: string[] = [];
  for (const { file, text } of sources) {
    for (const m of text.matchAll(pattern)) out.push(`${file}: ${m[0].trim()}`);
  }
  return out;
}

// ── Type ───────────────────────────────────────────────────────────

/** A class that sets a font size outside the named scale: in pixels or rem, or one of Tailwind's own names. */
const OFF_SCALE_SIZE = /(?<![\w-])text-(?:\[\d+(?:\.\d+)?(?:px|rem)\]|xs|sm|base|lg|xl|[2-9]xl)(?![\w-])/g;

test('the type scale is eight named sizes, from 12px to 28px', () => {
  assert.deepEqual(Object.values(TYPE_SCALE), ['12px', '13px', '14px', '15px', '16px', '17px', '20px', '28px']);
});

test('the check sees a size outside the scale however it is written', () => {
  for (const sample of ['text-[16px]', 'text-[1.1rem]', 'text-xs', 'text-sm', 'text-base', 'text-lg', 'text-2xl', "'h-8 text-sm px-2'"]) {
    assert.ok(sample.match(OFF_SCALE_SIZE), sample);
  }
  // The scale's own names, a size relative to its text, and words that only look like one.
  for (const fine of ['text-small', 'text-body-lg', 'text-title', 'text-[0.875em]', 'text-shodh-text', 'context-sm', 'text-smaller']) {
    assert.equal(fine.match(OFF_SCALE_SIZE), null, fine);
  }
});

test('no class sets a font size outside the scale', () => {
  assert.deepEqual(found(code, OFF_SCALE_SIZE), []);
});

// ── Spacing ────────────────────────────────────────────────────────

/** A padding, margin or gap written in pixels of its own. */
const OFF_SCALE_SPACE = /(?<![\w-])-?(?:p|px|py|pt|pb|pl|pr|ps|pe|m|mx|my|mt|mb|ml|mr|ms|me|gap|gap-x|gap-y|space-x|space-y)-\[\d+(?:\.\d+)?px\]/g;

test("spacing is Tailwind's scale and two named measures", () => {
  const block = /spacing:\s*\{([^}]*)\}/.exec(tailwind);
  assert.ok(block, 'tailwind.config.js extends spacing');
  const configured: Record<string, string> = {};
  for (const m of block[1].matchAll(/['"]?([\w.-]+)['"]?\s*:\s*['"]([^'"]+)['"]/g)) configured[m[1]] = m[2];
  assert.deepEqual(configured, { ...EXTRA_SPACING });
  for (const sample of ['px-[18px]', 'mt-[3px]', '-mr-[7px]', 'gap-[7px]', 'pb-[84px]']) assert.ok(sample.match(OFF_SCALE_SPACE), sample);
  for (const fine of ['px-4.5', 'pb-page-end', 'mt-0.5', 'w-[18px]', 'top-[3px]', 'gap-1.5']) assert.equal(fine.match(OFF_SCALE_SPACE), null, fine);
});

test('no class writes a padding, margin or gap in pixels of its own', () => {
  assert.deepEqual(found(code, OFF_SCALE_SPACE), []);
});

// ── Colour ─────────────────────────────────────────────────────────

/** Other products' marks, drawn in their own colours. */
const BRAND_MARKS = new Set([
  'src/features/connectors/SlackLogo.tsx',
  'src/features/connectors/TelegramLogo.tsx',
  'src/features/connectors/GitHubLogo.tsx',
  'src/features/connectors/TelegramLink.tsx',
]);

/** A colour class from Tailwind's own palette rather than a token. */
const PALETTE_CLASS =
  /(?<![\w-])(?:bg|text|border|ring|fill|stroke|from|to|via|divide|outline|decoration|placeholder|accent|caret|shadow)-(?:gray|slate|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)-\d{2,3}(?![\w-])/g;

test('no component writes a colour of its own: hex values stay in the tokens', () => {
  const hex = found(views.filter(v => !BRAND_MARKS.has(v.file)), /#[0-9a-fA-F]{6}\b|#[0-9a-fA-F]{3}\b(?![\w-])/g)
    // A URL fragment or an id is not a colour.
    .filter(line => !/#[0-9a-fA-F]{3,6}[\w-]/.test(line));
  assert.deepEqual(hex, []);
  for (const mark of BRAND_MARKS) assert.ok(views.some(v => v.file === mark), `${mark} is a source file`);
});

test("no class takes a colour from Tailwind's palette", () => {
  for (const sample of ['text-gray-600', 'bg-red-50', 'border-orange-500', 'dark:bg-gray-900', 'hover:bg-red-500/10']) assert.ok(sample.match(PALETTE_CLASS), sample);
  for (const fine of ['text-shodh-text', 'bg-shodh-error/10', 'bg-white', 'bg-black/40', 'border-shodh-border-strong']) assert.equal(fine.match(PALETTE_CLASS), null, fine);
  assert.deepEqual(found(code, PALETTE_CLASS), []);
});

// ── Focus ──────────────────────────────────────────────────────────

/** `outline-none` on its own (not `focus-visible:outline-none`, which is half of a focus style). */
const BARE_OUTLINE_NONE = /(?<![\w:-])outline-none(?![\w-])/g;
/** A focus style by a shared constant's name, or a target only code focuses (a heading scrolled to). */
const NAMED_OR_CODE_ONLY = /FOCUS_|tabIndex=\{-1\}/;
/**
 * Where the focus is drawn by something else, each with the style that draws it: a field
 * inside a box that lights up, and dialogs, whose panel only holds the focus for what is in it.
 */
const DRAWN_ELSEWHERE: Record<string, RegExp> = {
  'src/features/agent/AgentComposer.tsx': /has-\[textarea:focus-visible\]:ring-1/,
  'src/features/tasks/fields.tsx': /focus-within:border-shodh-border-strong/,
  'src/features/research/SnippetHost.tsx': /<Dialog\.Content[^>]*DIALOG_CLASS/,
};

test('everything that removes the outline shows focus another way', () => {
  const missing: string[] = [];
  for (const { file, text } of views) {
    const tags = openingTags(text, ['[A-Za-z][\\w.]*']);
    for (const m of text.matchAll(BARE_OUTLINE_NONE)) {
      const tag = tags.find(t => t.at <= m.index && m.index < t.at + t.tag.length);
      // In a tag: that element. Outside one (a class constant): its own line.
      const scope = tag ? tag.tag : text.slice(text.lastIndexOf('\n', m.index) + 1, text.indexOf('\n', m.index));
      if (FOCUS_STYLE.test(scope) || NAMED_OR_CODE_ONLY.test(scope)) continue;
      if (tag?.name === 'Dialog.Content') continue;
      if (DRAWN_ELSEWHERE[file]?.test(text)) continue;
      missing.push(`${file}:${lineOf(text, m.index)}`);
    }
  }
  // Taking the outline away is not a focus style, however it is written.
  assert.ok(!FOCUS_STYLE.test('outline-none focus:outline-none') && !FOCUS_STYLE.test('focus-visible:outline-none'));
  assert.ok(FOCUS_STYLE.test('focus-visible:outline-none focus-visible:ring-2'));
  assert.deepEqual(missing, []);
  // The line under a boxless field is in the ring's colour, whose contrast is tested.
  assert.equal(FOCUS_UNDERLINE, 'focus-visible:shadow-[inset_0_-2px_0_var(--c-accent-text)]');
  assert.match(tailwind, /ring: token\('accent-text'\)/);
});

// ── Targets ────────────────────────────────────────────────────────

/** A height or width under 24px (Tailwind's `h-5` is 20px). */
const SMALL_SIDE = /(?<![\w:-])(?:h|w|size)-(?:0\.5|1|1\.5|2|2\.5|3|3\.5|4|5|\[(?:\d|1\d|2[0-3])px\])(?![\w.-])/;

test('a control drawn under 24px still takes a click in a 24px square', () => {
  const small: string[] = [];
  for (const { file, text } of views) {
    for (const { at, tag } of openingTags(text, ['button', 'a'])) {
      if (SMALL_SIDE.test(tag) && !/(?<![\w-])target-24(?![\w-])/.test(tag)) small.push(`${file}:${lineOf(text, at)}`);
    }
  }
  assert.deepEqual(small, []);
  assert.match(css, /\.target-24::after \{[^}]*width: max\(100%, 24px\);\s*height: max\(100%, 24px\);/);
  for (const sample of ['<button className="w-5 h-5">', "<button className={cn('w-9 h-5 rounded-full')}>", '<a className="h-[18px]">']) {
    assert.ok(SMALL_SIDE.test(sample), sample);
  }
  for (const fine of ['<button className="h-6 w-6">', '<button className="h-7 px-2"><X className="w-3.5 h-3.5" />', '<button className="min-h-9">']) {
    assert.ok(!SMALL_SIDE.test(openingTags(fine, ['button'])[0].tag), fine);
  }
});

// ── Labels ─────────────────────────────────────────────────────────

test('a button laid out as one row never wraps its label', () => {
  assert.match(css, /:where\(button\.inline-flex, a\.inline-flex\) \{\s*white-space: nowrap;\s*\}/);
  // The Library's folder card, where "Browse files" and "Re-index" wrapped: both are such buttons.
  const library = code.find(c => c.file === 'src/features/library/LibraryView.tsx');
  assert.ok(library);
  assert.match(library.text, /const QUIET_BUTTON = cn\(\s*'h-8 px-2\.5 inline-flex /);
});

// ── Motion ─────────────────────────────────────────────────────────

test('loops stop, and durations go to zero, for reduced motion', () => {
  const loops = found(views, /^.*animate-(?:spin|pulse|bounce|ping).*$/gm).filter(line => !line.includes('motion-reduce:animate-none'));
  assert.deepEqual(loops, []);
  assert.match(css, /@media \(prefers-reduced-motion: reduce\) \{\s*:root \{\s*--dur-micro: 0ms;\s*--dur-panel: 0ms;\s*--dur-screen: 0ms;\s*--dur-exit: 0ms;/);
  const main = code.find(c => c.file === 'src/main.tsx');
  assert.ok(main);
  assert.match(main.text, /<MotionConfig reducedMotion="user">/);
});
