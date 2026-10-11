/**
 * Suggested next questions of a side answer. Side questions ask the model
 * to end its answer with
 *
 *   ```followups
 *   ["question 1", "question 2", "question 3"]
 *   ```
 *
 * The block is removed from what is shown and stored, and its questions
 * become chips under the answer. Parsing never throws: a malformed block is
 * removed and yields no chips.
 *
 * Pure module, unit-tested with Node (`app/tests/focusFollowups.test.ts`).
 */

/** Chips shown under one answer. */
export const MAX_FOLLOWUPS = 3;
/** Longest suggested question kept (longer ones are dropped, not cut). */
export const MAX_FOLLOWUP_CHARS = 160;
/** A block larger than this is not parsed (still removed). */
export const MAX_FOLLOWUP_BLOCK_CHARS = 4_000;

/** The instruction placed in each side question (never in the main conversation). */
export const FOLLOWUPS_INSTRUCTION = [
  'Answer format: after your answer, end with exactly one fenced block of up to three short follow-up questions the reader might ask next, as a JSON array of strings:',
  '```followups',
  '["…", "…", "…"]',
  '```',
].join('\n');

const BLOCK = /(^|\n)[ \t]*(`{3,}|~{3,})[ \t]*followups[ \t]*\n([\s\S]*?)(?:\n[ \t]*\2[ \t]*(?=\n|$)|$)/g;

function clean(value: unknown): string | null {
  if (typeof value !== 'string') return null;
  const text = value.replace(/\s+/g, ' ').trim();
  if (!text || Array.from(text).length > MAX_FOLLOWUP_CHARS) return null;
  return text;
}

/** Questions of one block payload; empty when it is not a JSON array of strings. */
export function parseFollowupPayload(payload: string): string[] {
  if (payload.length > MAX_FOLLOWUP_BLOCK_CHARS) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(payload.trim());
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  const out: string[] = [];
  const seen = new Set<string>();
  for (const item of parsed) {
    const text = clean(item);
    if (!text || seen.has(text.toLowerCase())) continue;
    seen.add(text.toLowerCase());
    out.push(text);
    if (out.length === MAX_FOLLOWUPS) break;
  }
  return out;
}

/**
 * The answer without its followups block(s), and the questions of the last
 * block. While an answer streams, a block that is still open (or an opening
 * line still being typed) is hidden too.
 */
export function splitFollowups(text: string): { body: string; followups: string[] } {
  let followups: string[] = [];
  let body = text.replace(BLOCK, (_match, lead: string, _fence: string, payload: string) => {
    followups = parseFollowupPayload(payload);
    return lead;
  });
  // An opening line still arriving: "```fol".
  const lastBreak = body.lastIndexOf('\n');
  const lastLine = body.slice(lastBreak + 1).trim();
  const partial = /^(`{3,}|~{3,})([a-z]+)$/.exec(lastLine);
  if (partial && 'followups'.startsWith(partial[2])) body = body.slice(0, Math.max(0, lastBreak));
  return { body: body.replace(/\s+$/, ''), followups };
}

/** Just the answer text, without followups. */
export function stripFollowups(text: string): string {
  return splitFollowups(text).body;
}

/** Stored suggestions of a turn: strings only, within the caps. */
export function readFollowups(value: unknown): string[] | undefined {
  if (!Array.isArray(value)) return undefined;
  const out = value.map(clean).filter((s): s is string => s !== null).slice(0, MAX_FOLLOWUPS);
  return out.length > 0 ? out : undefined;
}
