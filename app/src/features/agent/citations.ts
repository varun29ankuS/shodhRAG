import { parsePageSpan, samePathSpelling } from '../ask/searchResults';
import type { SearchHit } from '../ask/types';
import type { Passage } from './reducer';

/**
 * Passages of a run as citation targets. `number` is the passage's run-wide
 * `n`, which is exactly what the model writes as `[n]`. Web sources carry
 * their address in `url`, which is how the UI tells them apart from
 * document citations; an indexed Slack thread carries its permalink there
 * too, so its citation opens Slack, never a local path.
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
    page: p.web ? null : parsePageSpan(p.page),
    lineRange: null,
    // A web source's address, or an indexed Slack thread's permalink: both open in the browser.
    url: p.web ? p.path : (p.url ?? null),
    section: p.web ? null : (p.section ?? null),
    regions: p.web ? null : (p.regions ?? null),
    ...(!p.web && samePathSpelling(p.path, p.displayPath) ? { displayPath: p.displayPath } : {}),
  }));
}
