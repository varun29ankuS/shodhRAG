/**
 * Keyboard map of the document viewer and file browser, and plain-text
 * find over PDF page text.
 *
 * Pure module (no runtime imports) so it is unit-tested directly with Node
 * (`app/tests/viewerKeys.test.ts`).
 */

export type ViewerCommand =
  | 'nextPage'
  | 'prevPage'
  | 'firstPage'
  | 'lastPage'
  | 'zoomIn'
  | 'zoomOut'
  | 'fitWidth'
  | 'toggleFocus'
  | 'find'
  | 'findNext'
  | 'findPrev';

export interface KeyInput {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  /** Focus is in a text field, select or contenteditable element. */
  editable: boolean;
}

/**
 * The command a key press means, or null when it should keep its normal
 * behaviour.
 *
 * - Ctrl/⌘+F find, F3 / Ctrl/⌘+G next match (Shift: previous).
 * - Ctrl/⌘ with = + - 0 zoom the document instead of the whole window.
 * - Without modifiers (and not while typing): PageUp/PageDown previous/next
 *   page, Home/End first/last page, + = / - _ zoom, 0 fit width, F focus mode.
 */
export function viewerCommand(input: KeyInput): ViewerCommand | null {
  const { key, altKey, shiftKey, editable } = input;
  const mod = input.ctrlKey || input.metaKey;
  if (altKey) return null;

  if (mod) {
    const lower = key.length === 1 ? key.toLowerCase() : key;
    if (lower === 'f' && !shiftKey) return 'find';
    if (lower === 'g') return shiftKey ? 'findPrev' : 'findNext';
    if (key === '=' || key === '+') return 'zoomIn';
    if (key === '-' || key === '_') return 'zoomOut';
    if (key === '0') return 'fitWidth';
    return null;
  }

  if (key === 'F3') return shiftKey ? 'findPrev' : 'findNext';
  if (editable) return null;

  switch (key) {
    case 'PageDown':
      return 'nextPage';
    case 'PageUp':
      return 'prevPage';
    case 'Home':
      return shiftKey ? null : 'firstPage';
    case 'End':
      return shiftKey ? null : 'lastPage';
    case '+':
    case '=':
      return 'zoomIn';
    case '-':
    case '_':
      return 'zoomOut';
    case '0':
      return shiftKey ? null : 'fitWidth';
    case 'f':
    case 'F':
      return 'toggleFocus';
    default:
      return null;
  }
}

export interface TextSpan {
  start: number;
  end: number;
}

/**
 * Every occurrence of `query` in `text`, ignoring case and treating any run
 * of whitespace as one space. Offsets index the original `text`.
 */
export function findInText(text: string, query: string): TextSpan[] {
  const needle = normalizeQuery(query);
  if (!needle) return [];
  // Normalised haystack plus the original offset of each of its characters.
  let hay = '';
  const origin: number[] = [];
  let inSpace = false;
  for (let i = 0; i < text.length; i += 1) {
    const ch = text[i];
    if (/\s/.test(ch)) {
      if (!inSpace) {
        hay += ' ';
        origin.push(i);
        inSpace = true;
      }
      continue;
    }
    inSpace = false;
    hay += lowerSameLength(ch);
    origin.push(i);
  }
  const out: TextSpan[] = [];
  let from = 0;
  for (;;) {
    const at = hay.indexOf(needle, from);
    if (at < 0) break;
    const last = at + needle.length - 1;
    out.push({ start: origin[at], end: origin[last] + 1 });
    from = at + needle.length;
  }
  return out;
}

function normalizeQuery(query: string): string {
  let out = '';
  for (const ch of query.trim().replace(/\s+/g, ' ')) out += lowerSameLength(ch);
  return out;
}

/** Lowercase that never changes the length (keeps offsets aligned). */
function lowerSameLength(ch: string): string {
  const lower = ch.toLowerCase();
  return lower.length === ch.length ? lower : ch;
}
