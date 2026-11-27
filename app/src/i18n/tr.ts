/**
 * The UI's words, from one catalogue (`en.ts`): `tr(key)` is a label, a title or a line,
 * with `{name}` filled from `vars`; `trCount(key, n)` picks the plural form the user's
 * locale gives `n` (`key.one`, `key.other`, ...) and fills `{count}`. English is the only
 * language; a second one is a second catalogue with the same keys. Tests swap the
 * catalogue for a pseudo-locale (`app/tests/i18nReady.test.ts`).
 *
 * No React: the Node tests load it.
 */
import { en } from './en.ts';
import type { MessageKey } from './en.ts';
import { formatNumber, pluralCategory } from './format.ts';

export type { MessageKey } from './en.ts';

type Catalogue = Readonly<Record<MessageKey, string>>;

let active: Catalogue = en;

/** Use `catalogue` for every message (a test's pseudo-locale), or English again with none. */
export function setCatalogue(catalogue?: Catalogue): void {
  active = catalogue ?? en;
}

/** `{name}` in `text` replaced by `vars.name` (numbers in the user's locale); others kept. */
function fill(text: string, vars?: Readonly<Record<string, string | number>>): string {
  if (!vars) return text;
  return text.replace(/\{(\w+)\}/g, (whole, name: string) => {
    const value = vars[name];
    if (value === undefined) return whole;
    return typeof value === 'number' ? formatNumber(value) : value;
  });
}

/** The message `key`, with `{name}` filled from `vars`. */
export function tr(key: MessageKey, vars?: Readonly<Record<string, string | number>>): string {
  return fill(active[key], vars);
}

/**
 * The plural form of `base` for `n`: `base.one`, `base.other`, ... as the user's locale
 * selects, falling back to `base.other`; `{count}` is `n`.
 */
export function trCount(base: string, n: number, vars?: Readonly<Record<string, string | number>>): string {
  const forms = active as Readonly<Record<string, string>>;
  const text = forms[`${base}.${pluralCategory(n)}`] ?? forms[`${base}.other`];
  if (text === undefined) throw new Error(`No message ${base}.other`);
  return fill(text, { count: n, ...vars });
}
