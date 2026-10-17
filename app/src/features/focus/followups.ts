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
 * Models do not always keep the tag: some write a "Follow-ups:" label and a
 * ```json (or untagged) fence instead. That variant is accepted only where it
 * cannot be real content: it is the last block of the answer, nothing but
 * whitespace follows it, and its payload is a JSON array of 1 to
 * MAX_TOLERATED_ITEMS short strings that is either introduced by a follow-ups
 * label or made only of questions. A label line right before a removed block
 * is removed with it.
 *
 * Pure module, unit-tested with Node (`app/tests/focusFollowups.test.ts`).
 */

/** Chips shown under one answer. */
export const MAX_FOLLOWUPS = 3;
/** Longest suggested question kept (longer ones are dropped, not cut). */
export const MAX_FOLLOWUP_CHARS = 160;
/** A block larger than this is not parsed (still removed). */
export const MAX_FOLLOWUP_BLOCK_CHARS = 4_000;
/** Most items an untagged or ```json block may hold and still be read as follow-ups. */
export const MAX_TOLERATED_ITEMS = 5;

/** The instruction placed in each side question (never in the main conversation). */
export const FOLLOWUPS_INSTRUCTION = [
  'Answer format: after your answer, end with exactly one fenced block of up to three short follow-up questions the reader might ask next, as a JSON array of strings:',
  '```followups',
  '["…", "…", "…"]',
  '```',
].join('\n');

const BLOCK = /(^|\n)[ \t]*(`{3,}|~{3,})[ \t]*followups[ \t]*\n([\s\S]*?)(?:\n[ \t]*\2[ \t]*(?=\n|$)|$)/g;
/**
 * A follow-ups label on a line of its own: "Follow-ups:", "**Follow-up questions:**",
 * "### Follow-up questions", "Suggested follow-ups:", "Suggested questions:" …
 */
const LABEL = /^[ \t]*(?:#{1,6}[ \t]+)?(?:\*\*|__)?[ \t]*(?:suggested[ \t]+(?:follow[- ]?ups?(?:[ \t]+questions)?|questions)|follow[- ]?ups?(?:[ \t]+questions)?)[ \t]*:?[ \t]*(?:\*\*|__)?[ \t]*:?[ \t]*$/i;
/** A fence opening line: its marker and the first word of its info string. */
const FENCE_OPEN = /^[ \t]*(`{3,}|~{3,})[ \t]*([^\s`]*)[^`]*$/;
const FENCE_CLOSE = /^[ \t]*(`{3,}|~{3,})[ \t]*$/;

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

/** The last fenced block of a text, closed or still open. */
interface LastFence {
  /** Offset of its opening line. */
  start: number;
  /** First word of the info string, lowercased ('' when untagged). */
  info: string;
  payload: string;
  closed: boolean;
  /** Text after the closing line ('' for an open block). */
  after: string;
}

function lastFence(text: string): LastFence | null {
  let last: LastFence | null = null;
  let open: { start: number; marker: string; info: string; payloadStart: number } | null = null;
  let pos = 0;
  for (;;) {
    const nl = text.indexOf('\n', pos);
    const end = nl === -1 ? text.length : nl;
    const line = text.slice(pos, end);
    if (open) {
      const close = FENCE_CLOSE.exec(line);
      if (close && close[1][0] === open.marker[0] && close[1].length >= open.marker.length) {
        last = {
          start: open.start,
          info: open.info,
          payload: text.slice(open.payloadStart, Math.max(open.payloadStart, pos - 1)),
          closed: true,
          after: text.slice(end),
        };
        open = null;
      }
    } else {
      const opening = FENCE_OPEN.exec(line);
      if (opening) open = { start: pos, marker: opening[1], info: opening[2].toLowerCase(), payloadStart: end + 1 };
    }
    if (nl === -1) break;
    pos = nl + 1;
  }
  if (open) {
    last = { start: open.start, info: open.info, payload: text.slice(Math.min(open.payloadStart, text.length)), closed: false, after: '' };
  }
  return last;
}

/** The text before `end`, without a follow-ups label line that ends it. */
function withoutLabel(text: string, end: number): { body: string; labelled: boolean } {
  const before = text.slice(0, end).replace(/\s+$/, '');
  const lineStart = before.lastIndexOf('\n') + 1;
  if (!LABEL.test(before.slice(lineStart))) return { body: before, labelled: false };
  return { body: before.slice(0, lineStart), labelled: true };
}

/** The items of a payload that is a JSON array of 1 to MAX_TOLERATED_ITEMS short strings, else null. */
function toleratedItems(payload: string): string[] | null {
  if (payload.length > MAX_FOLLOWUP_BLOCK_CHARS) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(payload.trim());
  } catch {
    return null;
  }
  if (!Array.isArray(parsed) || parsed.length === 0 || parsed.length > MAX_TOLERATED_ITEMS) return null;
  const items = parsed.map(clean);
  return items.every((item): item is string => item !== null) ? items : null;
}

/**
 * A trailing ```json or untagged follow-ups block (see the module comment):
 * the text without it (and its label) plus its questions, or null when the
 * last block is not one. While streaming, an open such block after a
 * follow-ups label is hidden.
 */
function splitToleratedBlock(text: string): { body: string; followups: string[] } | null {
  const fence = lastFence(text);
  if (!fence || fence.after.trim() !== '') return null;
  const tagOk = fence.info === '' || fence.info === 'json';
  // An open fence whose tag is still being typed: "```js".
  const tagTyping = !fence.closed && fence.payload === '' && 'json'.startsWith(fence.info);
  if (!tagOk && !tagTyping) return null;
  const { body, labelled } = withoutLabel(text, fence.start);
  if (!fence.closed) return labelled ? { body, followups: [] } : null;
  const items = toleratedItems(fence.payload);
  if (!items) return null;
  if (!labelled && !items.every(item => item.endsWith('?'))) return null;
  return { body, followups: parseFollowupPayload(fence.payload) };
}

/**
 * The answer without its followups block(s), and the questions of the last
 * block. While an answer streams, a block that is still open (or an opening
 * line still being typed) is hidden too.
 */
export function splitFollowups(text: string): { body: string; followups: string[] } {
  let followups: string[] = [];
  let tagged = false;
  let body = text.replace(BLOCK, (_match, lead: string, _fence: string, payload: string) => {
    tagged = true;
    followups = parseFollowupPayload(payload);
    return lead;
  });
  // An opening line still arriving: "```fol".
  const lastBreak = body.lastIndexOf('\n');
  const lastLine = body.slice(lastBreak + 1).trim();
  const partial = /^(`{3,}|~{3,})([a-z]+)$/.exec(lastLine);
  if (partial && 'followups'.startsWith(partial[2])) {
    tagged = true;
    body = body.slice(0, Math.max(0, lastBreak));
  }
  if (tagged) {
    // A label written above the tagged block would be left dangling at the end.
    return { body: withoutLabel(body, body.length).body.replace(/\s+$/, ''), followups };
  }
  const tolerated = splitToleratedBlock(body);
  if (tolerated) return { body: tolerated.body.replace(/\s+$/, ''), followups: tolerated.followups };
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
