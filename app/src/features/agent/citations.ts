import { parsePageSpan } from '../ask/searchResults';
import type { SearchHit } from '../ask/types';
import type { Passage } from './reducer';

/**
 * Passages of a run as citation targets. `number` is the passage's run-wide
 * `n`, which is exactly what the model writes as `[n]`.
 */
export function passageHits(passages: readonly Passage[]): SearchHit[] {
  return passages.map(p => ({
    number: p.n,
    sourceFile: p.path,
    fileName: p.file,
    title: p.heading ? `${p.file} · ${p.heading}` : p.file,
    text: p.text,
    snippet: p.text.slice(0, 200),
    score: p.score,
    page: parsePageSpan(p.page),
    lineRange: null,
    url: null,
  }));
}
