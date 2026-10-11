/**
 * Tag chip input parsing. Pure module, unit-tested with Node
 * (`app/tests/taskTags.test.ts`).
 */

/** Longest tag kept; longer input is truncated rather than rejected. */
export const MAX_TAG_LENGTH = 40;

/** Characters that end a tag while typing (besides Enter). */
export const TAG_SEPARATORS = /[,;\n]/;

function normalize(raw: string): string {
  return raw
    .trim()
    .replace(/^#+/, '')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, MAX_TAG_LENGTH)
    .trim();
}

/**
 * Tags in `input`, split on commas, semicolons and newlines. Leading `#` is
 * dropped, inner whitespace collapsed, empties removed and duplicates
 * (case-insensitive) kept once, first spelling wins.
 */
export function parseTags(input: string): string[] {
  const seen = new Set<string>();
  const tags: string[] = [];
  for (const part of input.split(TAG_SEPARATORS)) {
    const tag = normalize(part);
    if (!tag || seen.has(tag.toLowerCase())) continue;
    seen.add(tag.toLowerCase());
    tags.push(tag);
  }
  return tags;
}

/** `existing` plus the new tags in `input`, without case-insensitive duplicates. */
export function addTags(existing: readonly string[], input: string): string[] {
  const seen = new Set(existing.map(t => t.toLowerCase()));
  const next = [...existing];
  for (const tag of parseTags(input)) {
    if (seen.has(tag.toLowerCase())) continue;
    seen.add(tag.toLowerCase());
    next.push(tag);
  }
  return next;
}

/** `existing` without `tag` (exact match). */
export function removeTag(existing: readonly string[], tag: string): string[] {
  return existing.filter(t => t !== tag);
}

/** True when two tag lists hold the same tags in the same order. */
export function sameTags(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((t, i) => t === b[i]);
}

/**
 * Split typed text at the last separator: complete tags before it, and the
 * remainder that stays in the input as the draft.
 */
export function splitDraft(text: string): { complete: string; draft: string } {
  let last = -1;
  for (let i = 0; i < text.length; i++) if (TAG_SEPARATORS.test(text[i])) last = i;
  return last < 0 ? { complete: '', draft: text } : { complete: text.slice(0, last), draft: text.slice(last + 1) };
}
