/**
 * The app ready for a second language (spec 4.4, gates in 5): physical CSS gone, no
 * locale pinned outside the helpers, the shell's, Ask's and Settings' words in the
 * catalogue, and a pseudo-locale 30% longer that every fixed-width button still fits.
 * RTL is made safe here, not tested in a right-to-left locale. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/i18nReady.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { join, relative } from 'node:path';
import ts from 'typescript';
import { en } from '../src/i18n/en.ts';
import type { MessageKey } from '../src/i18n/en.ts';
import { setCatalogue, tr, trCount } from '../src/i18n/tr.ts';
import { formatDate, formatNumber, isoDate, setFormatLocale } from '../src/i18n/format.ts';
import { appDir, files, srcDir } from './jsxScan.ts';

const where = (file: string) => relative(srcDir, file).replace(/\\/g, '/');
const sources = files(srcDir, /\.(ts|tsx)$/).map(file => ({ file: where(file), text: readFileSync(file, 'utf8') }));

/** The migrated folders and files: the shell, Ask and Settings. */
const SETTINGS_FILES = [
  'components/SearchSettings.tsx', 'components/PrivacySettings.tsx', 'components/BackgroundSettings.tsx',
  'components/NotificationSettings.tsx', 'components/MemorySettings.tsx', 'components/AnswerCheckSettings.tsx',
  'components/TableModelSettings.tsx', 'components/EvidenceTableSettings.tsx', 'components/ScanReaderSettings.tsx',
  'components/ToolsSettings.tsx', 'components/AppearanceSettings.tsx', 'components/SendKeySettings.tsx',
  'features/modelPicker/ModelSettingsSection.tsx', 'features/codeIntel/CodeSkillsSettings.tsx',
  'features/phone/PhoneSettings.tsx',
];
function migrated(file: string): boolean {
  if (!file.endsWith('.tsx')) return false;
  if (/^components\/shell\/[^/]+\.tsx$/.test(file) || /^features\/ask\/[^/]+\.tsx$/.test(file)) return true;
  return SETTINGS_FILES.includes(file);
}
const MIGRATED = sources.filter(s => migrated(s.file));

// ── Logical CSS ───────────────────────────────────────────────────

/** A physical utility: margins, paddings, insets, text alignment, side borders and corners. */
const PHYSICAL = /(?<=^|[\s"'`:(!])-?(?:(?:ml|mr|pl|pr)-(?:auto|px|full|\d[\d./]*|\[[^\]\s]+\])|(?:left|right)-(?:auto|px|full|\d[\d./]*|\[[^\]\s]+\])|text-(?:left|right)|border-[lr](?:-[\w[\]/.-]+)?|rounded-(?:l|r|tl|tr|bl|br)(?:-[\w[\]/.-]+)?)(?![\w-])/;

test('no physical left or right utility in app/src; centring stays as it is either way', () => {
  const found: string[] = [];
  for (const { file, text } of sources) {
    text.split('\n').forEach((line, i) => {
      // left-1/2 with -translate-x-1/2 centres a box, the same in either direction.
      const unCentred = line.replace(/(?<![\w-])left-1\/2(?![\w-])(?=.*-translate-x-1\/2)/, '');
      const m = PHYSICAL.exec(unCentred);
      if (m) found.push(`${file}:${i + 1}: ${m[0]}`);
    });
  }
  assert.deepEqual(found, []);
  for (const bad of ['"ml-2"', "'pl-3 x'", ' text-left', ' border-l-2', ' rounded-tl-md', ':right-0', ' -ml-1.5', ' left-[3px]']) {
    assert.ok(PHYSICAL.test(bad), bad);
  }
  for (const fine of [' ms-2', ' ps-3', ' text-start', ' border-s-2', ' rounded-ss-md', ' end-0', ' rounded-lg', ' border-shodh-border', ' slide-in-from-right-8']) {
    assert.ok(!PHYSICAL.test(fine), fine);
  }
});

// ── Locale ────────────────────────────────────────────────────────

test('no number, date or plural pins a locale outside the helpers', () => {
  const pinned = /\.toLocale(?:Date|Time)?String\(\s*['"`]|new Intl\.[A-Za-z]+\(\s*['"`]|Intl\.[A-Za-z]+\(\s*['"`]/;
  const found = sources.filter(s => !s.file.startsWith('i18n/') && pinned.test(s.text)).map(s => s.file);
  assert.deepEqual(found, []);
});

test('the helpers write numbers and dates in the locale asked for, and an ISO date from its parts', () => {
  try {
    setFormatLocale('de-DE');
    assert.equal(formatNumber(1204.5), '1.204,5');
    setFormatLocale('en-US');
    assert.equal(formatNumber(1204), '1,204');
    assert.equal(formatDate(Date.UTC(2026, 10, 4, 12), { day: 'numeric', month: 'short', timeZone: 'UTC' }), 'Nov 4');
  } finally {
    setFormatLocale();
  }
  assert.equal(isoDate(Date.UTC(2026, 10, 26, 23, 30), 'UTC'), '2026-11-26');
  // The day where it is: 23:30 UTC is already the 27th in Kolkata.
  assert.equal(isoDate(Date.UTC(2026, 10, 26, 23, 30), 'Asia/Kolkata'), '2026-11-27');
});

// ── Strings ───────────────────────────────────────────────────────

const LABELS = new Set(['aria-label', 'title', 'placeholder', 'alt']);

/** Words written in the migrated files' JSX, not read from the catalogue. */
function writtenWords(file: string, text: string): string[] {
  const sf = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const out: string[] = [];
  const visit = (node: ts.Node) => {
    if (node.kind === ts.SyntaxKind.JsxText && /[A-Za-z]/.test(node.getText(sf))) {
      out.push(`${file}: text "${node.getText(sf).trim().slice(0, 40)}"`);
    } else if (ts.isJsxAttribute(node) && node.initializer && ts.isStringLiteral(node.initializer)) {
      const name = node.name.getText(sf);
      if (LABELS.has(name) && /[A-Za-z]/.test(node.initializer.text)) out.push(`${file}: ${name}="${node.initializer.text}"`);
    }
    ts.forEachChild(node, visit);
  };
  visit(sf);
  return out;
}

test('the shell, Ask and Settings read their words from the catalogue', () => {
  assert.ok(MIGRATED.length >= 40, `${MIGRATED.length} migrated files`);
  const written = MIGRATED.flatMap(({ file, text }) => writtenWords(file, text));
  assert.deepEqual(written, []);
  // The check sees words where they are written.
  assert.equal(writtenWords('x.tsx', '<p title="Hi">Hello</p>').length, 2);
  assert.equal(writtenWords('x.tsx', "<p title={tr('a.b')}>{tr('a.c')} · </p>").length, 0);
});

test('every key read exists, and every key in the catalogue is read', () => {
  const used = new Set<string>();
  for (const { text } of sources) for (const m of text.matchAll(/\btr\('([\w.]+)'/g)) used.add(m[1]);
  const keys = Object.keys(en);
  assert.deepEqual([...used].filter(k => !(k in en)), []);
  assert.deepEqual(keys.filter(k => !used.has(k)), []);
  // Keys say where they are read: area, file, words.
  for (const key of keys) assert.match(key, /^(shell|ask|settings)\.[a-z]\w*\.[a-z]\w*$/, key);
});

test('a message fills its names; a count picks its plural form', () => {
  const key = Object.keys(en)[0] as MessageKey;
  try {
    setCatalogue({ ...en, [key]: 'Read {file} in {count} s', 'x.files.one': '{count} file', 'x.files.other': '{count} files' } as typeof en);
    assert.equal(tr(key, { file: 'a.pdf', count: 1204 }), `Read a.pdf in ${formatNumber(1204)} s`);
    assert.equal(tr(key), 'Read {file} in {count} s');
    assert.equal(trCount('x.files', 1), '1 file');
    assert.equal(trCount('x.files', 3), '3 files');
    assert.equal(trCount('x.files', 0), '0 files');
  } finally {
    setCatalogue();
  }
});

// ── +30% text ─────────────────────────────────────────────────────

/** A pseudo-locale message: accented, and 30% longer (rounded up), as a longer language reads. */
export function pseudo(text: string): string {
  const ACCENT: Record<string, string> = { a: 'á', e: 'é', i: 'í', o: 'ó', u: 'ú', A: 'Á', E: 'É', I: 'Í', O: 'Ó', U: 'Ú', n: 'ñ', c: 'ç' };
  const accented = [...text].map(ch => ACCENT[ch] ?? ch).join('');
  const extra = Math.ceil(text.length * 0.3);
  return `${accented}${'~'.repeat(extra)}`;
}

const FONT_PX: Record<string, number> = { caption: 12, small: 13, body: 14, 'body-lg': 15, reading: 16, subhead: 17, title: 20, display: 28 };
/** An average glyph's width as a share of the font size: wide enough for accented Latin text. */
const GLYPH_EM = 0.62;

/** Buttons and chips with a fixed width class, labelled from the catalogue. */
function fixedWidthLabels(file: string, text: string): Array<{ where: string; width: number; padding: number; font: number; key: MessageKey }> {
  const sf = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const out: Array<{ where: string; width: number; padding: number; font: number; key: MessageKey }> = [];
  const visit = (node: ts.Node) => {
    // A field whose placeholder is read from the catalogue, in a field of a fixed width.
    if (ts.isJsxSelfClosingElement(node) && node.tagName.getText(sf) === 'input') {
      const props = node.attributes.properties.map(p => p.getText(sf));
      const cls = props.find(a => a.startsWith('className=')) ?? '';
      const w = /(?<![\w:-])w-(\d+(?:\.5)?|\[(\d+)px\])(?![\w.-])/.exec(cls);
      const holder = props.map(a => /^placeholder=\{tr\('([\w.]+)'\)\}$/.exec(a)).find(Boolean);
      if (w && holder) {
        const px = /(?<![\w:-])px-(\d+(?:\.5)?)(?![\w.-])/.exec(cls);
        const font = /(?<![\w:-])text-(caption|small|body-lg|body|reading|subhead|title|display)(?![\w-])/.exec(cls);
        out.push({
          where: `${file}:${sf.getLineAndCharacterOfPosition(node.getStart(sf)).line + 1}`,
          width: w[2] ? Number(w[2]) : Number(w[1]) * 4,
          padding: px ? Number(px[1]) * 4 * 2 : 0,
          font: font ? FONT_PX[font[1]] : FONT_PX.body,
          key: holder[1] as MessageKey,
        });
      }
    }
    if (ts.isJsxElement(node)) {
      const open = node.openingElement;
      const tag = open.tagName.getText(sf);
      const cls = open.attributes.properties.map(p => p.getText(sf)).find(a => a.startsWith('className=')) ?? '';
      const w = /(?<![\w:-])w-(\d+(?:\.5)?|\[(\d+)px\])(?![\w.-])/.exec(cls);
      const label = node.children.map(c => c.getText(sf)).join('').match(/^\s*\{tr\('([\w.]+)'\)\}\s*$/);
      if ((tag === 'button' || /chip/i.test(cls)) && w && label) {
        const width = w[2] ? Number(w[2]) : Number(w[1]) * 4;
        const px = /(?<![\w:-])px-(\d+(?:\.5)?)(?![\w.-])/.exec(cls);
        const font = /(?<![\w:-])text-(caption|small|body-lg|body|reading|subhead|title|display)(?![\w-])/.exec(cls);
        out.push({
          where: `${file}:${sf.getLineAndCharacterOfPosition(node.getStart(sf)).line + 1}`,
          width,
          padding: px ? Number(px[1]) * 4 * 2 : 0,
          font: font ? FONT_PX[font[1]] : FONT_PX.body,
          key: label[1] as MessageKey,
        });
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(sf);
  return out;
}

test('every fixed-width button and chip fits its label 30% longer', () => {
  const pseudoCatalogue = Object.fromEntries(Object.entries(en).map(([k, v]) => [k, pseudo(v)])) as unknown as typeof en;
  const tight: string[] = [];
  let checked = 0;
  try {
    setCatalogue(pseudoCatalogue);
    for (const { file, text } of MIGRATED) {
      for (const label of fixedWidthLabels(file, text)) {
        checked += 1;
        const needed = tr(label.key).length * label.font * GLYPH_EM;
        if (needed > label.width - label.padding) tight.push(`${label.where}: "${tr(label.key)}" needs ${Math.ceil(needed)}px, has ${label.width - label.padding}`);
      }
    }
  } finally {
    setCatalogue();
  }
  assert.deepEqual(tight, []);
  // It measured the migrated views' fixed-width labels (the find box's field among them).
  assert.ok(checked >= 1, `${checked} fixed-width labels`);
  // The check finds a fixed-width label and a tight one.
  const sample = fixedWidthLabels('x.tsx', "<button className=\"w-20 px-2 text-small\">{tr('k.a.b')}</button>");
  assert.deepEqual(sample.map(s => [s.width, s.padding, s.font]), [[80, 16, 13]]);
  assert.equal(pseudo('Open'), 'Ópéñ~~');
  assert.ok(pseudo('Set up Shodh').length >= Math.ceil('Set up Shodh'.length * 1.3));
});

test('buttons in the migrated views never wrap: the pseudo-locale grows them, it does not break them', () => {
  // Labels that grow 30% stay on one line (the 2a rule, \`whitespace-nowrap\` in the
  // shared button styles); this holds the rule where the catalogue labels them.
  const css = readFileSync(join(appDir, 'src', 'index.css'), 'utf8');
  assert.match(css, /button[^{]*\{[^}]*white-space: nowrap/);
});
