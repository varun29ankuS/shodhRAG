/**
 * Reading JSX sources in the Node tests, which have no DOM: the opening tags of a file, in
 * the order they are written (which is the order the browser tabs through them, since
 * nothing in the app sets a positive `tabIndex`).
 */
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
export const appDir = join(here, '..');
export const srcDir = join(appDir, 'src');

export interface Tag {
  /** The element or component's name. */
  name: string;
  /** Where it starts in the file. */
  at: number;
  /** The whole opening tag, to the `>` that closes it. */
  tag: string;
}

/** The opening tags named by `names` (regular expression sources) in `text`, in source order. */
export function openingTags(text: string, names: readonly string[]): Tag[] {
  const out: Tag[] = [];
  for (const m of text.matchAll(new RegExp(`<(${names.join('|')})(?![\\w.-])`, 'g'))) {
    let i = m.index + m[0].length;
    let depth = 0;
    let quote: string | null = null;
    for (; i < text.length; i++) {
      const c = text[i];
      if (quote) {
        if (c === quote) quote = null;
      } else if (c === '/' && text[i + 1] === '/') {
        // A comment between attributes: its apostrophes are not quotes.
        i = text.indexOf('\n', i);
        if (i < 0) break;
      } else if (c === '/' && text[i + 1] === '*') {
        i = text.indexOf('*/', i) + 1;
        if (i < 1) break;
      } else if (c === '"' || c === "'" || c === '`') quote = c;
      else if (c === '{') depth += 1;
      else if (c === '}') depth -= 1;
      else if (c === '>' && depth === 0) break;
    }
    out.push({ name: m[1], at: m.index, tag: text.slice(m.index, i + 1) });
  }
  return out;
}

export const lineOf = (text: string, at: number) => text.slice(0, at).split('\n').length;

/** Every file under `dir` whose name matches `pattern`. */
export function files(dir: string, pattern: RegExp): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) out.push(...files(path, pattern));
    else if (pattern.test(name)) out.push(path);
  }
  return out;
}

/** A file's path from the app folder, with forward slashes. */
export const where = (file: string) => relative(appDir, file).split('\\').join('/');

/** A source file of the app, by its path under `src`. */
export const source = (path: string) => readFileSync(join(srcDir, path), 'utf8');

/** Source without its comments, so a check reads code only. */
export const withoutComments = (text: string) => text.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '');

/** The elements the browser tabs to by themselves. */
export const NATIVE_STOPS = ['button', 'a', 'input', 'textarea', 'select', 'summary'] as const;

/** Whether an opening tag is taken out of the tab order. */
export const skipped = (tag: string) => /tabIndex=\{-1\}/.test(tag) || /\stype="hidden"/.test(tag);

/**
 * A style drawn on keyboard focus: a ring, a border, a line, a fill. `focus:outline-none`
 * and `focus-visible:outline-none` are not one: they only take the browser's outline away.
 */
export const FOCUS_STYLE = /(?:focus-visible|focus-within|focus):(?!outline-none)[\w[-]|\[&:focus-visible|has-\[[^\]]*focus/;

/** The browser's outline taken away. */
export const NO_OUTLINE = /(?<![\w-])outline-none(?![\w-])/;

/**
 * A tag's classes with the constants they name written out (`cn(iconButton, FOCUS_RING)`
 * becomes the text of both), two steps deep, from the same file.
 */
export function resolvedClasses(tag: string, fileText: string): string {
  const defined = (name: string) => new RegExp(String.raw`const ${name}\s*=([\s\S]*?);\n`).exec(fileText)?.[1] ?? '';
  let out = tag;
  for (const m of tag.matchAll(/\b([A-Za-z_]\w{2,})\b/g)) {
    const first = defined(m[1]);
    if (!first) continue;
    out += ` ${first}`;
    for (const inner of first.matchAll(/\b([A-Za-z_]\w{2,})\b/g)) out += ` ${defined(inner[1])}`;
  }
  return out;
}

/**
 * Whether a stop shows keyboard focus: by a focus style of its own (or of a constant it
 * names), or by the browser's outline, which it keeps unless it takes it away.
 */
export function showsFocus(tag: string, fileText: string): boolean {
  const classes = resolvedClasses(tag, fileText);
  // `FOCUS_UNDERLINE` (lib/focusStyles.ts) is imported where it is used: known by its name.
  return FOCUS_STYLE.test(classes) || tag.includes('FOCUS_UNDERLINE') || !NO_OUTLINE.test(classes);
}
