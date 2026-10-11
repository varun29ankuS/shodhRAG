/**
 * Command palette matching and ranking. Pure module (no runtime imports) so
 * it is unit-tested directly with Node (`app/tests/paletteFilter.test.ts`).
 */

export interface PaletteEntry {
  id: string;
  label: string;
  /** Extra words that match but are not shown. */
  keywords?: string;
  /** Section the entry is listed under; also its order among equal scores. */
  section: string;
}

function normalize(text: string): string {
  return text.toLocaleLowerCase().normalize('NFKD').replace(/[̀-ͯ]/g, '');
}

/** Words of `text`, split on anything that is not a letter or digit. */
function words(text: string): string[] {
  return normalize(text).split(/[^\p{L}\p{N}]+/u).filter(Boolean);
}

/**
 * Score one query token against an entry; 0 means no match.
 * Label matches outrank keyword matches; whole-label prefix outranks a word
 * prefix, which outranks a substring.
 */
function tokenScore(token: string, label: string, labelWords: string[], keywordWords: string[]): number {
  if (label.startsWith(token)) return 100;
  if (labelWords.some(w => w.startsWith(token))) return 70;
  if (label.includes(token)) return 40;
  if (keywordWords.some(w => w.startsWith(token))) return 25;
  if (keywordWords.some(w => w.includes(token))) return 10;
  return 0;
}

/**
 * Score an entry for a query: every query word must match somewhere, or the
 * entry is excluded (score 0). An empty query matches everything with score 1.
 */
export function scoreEntry(entry: PaletteEntry, query: string): number {
  const tokens = words(query);
  if (tokens.length === 0) return 1;
  const label = normalize(entry.label);
  const labelWords = words(entry.label);
  const keywordWords = words(entry.keywords ?? '');
  let total = 0;
  for (const token of tokens) {
    const score = tokenScore(token, label, labelWords, keywordWords);
    if (score === 0) return 0;
    total += score;
  }
  return total;
}

/**
 * Entries matching `query`. With an empty query the input order is kept;
 * otherwise entries are ranked by score, ties keeping the order of
 * `sectionOrder` and then input order.
 */
export function filterEntries<T extends PaletteEntry>(
  entries: readonly T[],
  query: string,
  sectionOrder: readonly string[],
): T[] {
  const rank = (section: string) => {
    const i = sectionOrder.indexOf(section);
    return i < 0 ? sectionOrder.length : i;
  };
  const scored = entries
    .map((entry, index) => ({ entry, index, score: scoreEntry(entry, query) }))
    .filter(s => s.score > 0);
  if (words(query).length === 0) {
    return scored
      .sort((a, b) => rank(a.entry.section) - rank(b.entry.section) || a.index - b.index)
      .map(s => s.entry);
  }
  return scored
    .sort((a, b) => b.score - a.score || rank(a.entry.section) - rank(b.entry.section) || a.index - b.index)
    .map(s => s.entry);
}

/** Group already-ordered entries into sections, keeping first-appearance order. */
export function groupBySection<T extends PaletteEntry>(entries: readonly T[]): { section: string; items: T[] }[] {
  const groups: { section: string; items: T[] }[] = [];
  const index = new Map<string, number>();
  for (const entry of entries) {
    const at = index.get(entry.section);
    if (at === undefined) {
      index.set(entry.section, groups.length);
      groups.push({ section: entry.section, items: [entry] });
    } else {
      groups[at].items.push(entry);
    }
  }
  return groups;
}
