/**
 * The answer's Markdown as the phone shows it: paragraphs, headings, list
 * items, quotes, code and tables, with bold, italics, inline code and the
 * citations `[n]` the answer gives. It reads text that is still streaming:
 * an unclosed code fence runs to the end, and a `**` not closed yet is
 * shown as nothing until it is.
 */

export type Inline =
  | { kind: 'text'; text: string; strong: boolean; em: boolean; code: boolean }
  | { kind: 'cite'; n: number };

export type Block =
  | { kind: 'p' | 'h' | 'quote'; inlines: Inline[] }
  | { kind: 'li'; inlines: Inline[]; marker: string }
  | { kind: 'code'; text: string }
  | { kind: 'table'; rows: Inline[][][] };

const FENCE = /^\s*(```|~~~)/;
const HEADING = /^\s{0,3}#{1,6}\s+(.*)$/;
const BULLET = /^\s*[-*+]\s+(.*)$/;
const NUMBERED = /^\s*(\d{1,3})[.)]\s+(.*)$/;
const QUOTE = /^\s*>\s?(.*)$/;
const RULE = /^\s*([-*_])(\s*\1){2,}\s*$/;
const TABLE_ROW = /^\s*\|.*\|\s*$/;
const TABLE_RULE = /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/;

/** The blocks of `markdown`. */
export function parseAnswer(markdown: string): Block[] {
  const lines = markdown.replace(/\r\n?/g, '\n').split('\n');
  const blocks: Block[] = [];
  let paragraph: string[] = [];
  let table: string[] = [];

  const endParagraph = () => {
    if (paragraph.length > 0) blocks.push({ kind: 'p', inlines: parseInline(paragraph.join(' ')) });
    paragraph = [];
  };
  const endTable = () => {
    if (table.length > 0) {
      const rows = table.filter(row => !TABLE_RULE.test(row)).map(row => splitRow(row).map(cell => parseInline(cell)));
      if (rows.length > 0) blocks.push({ kind: 'table', rows });
    }
    table = [];
  };

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (FENCE.test(line)) {
      endParagraph();
      endTable();
      const code: string[] = [];
      i++;
      while (i < lines.length && !FENCE.test(lines[i])) code.push(lines[i++]);
      blocks.push({ kind: 'code', text: code.join('\n') });
      continue;
    }
    if (TABLE_ROW.test(line)) {
      endParagraph();
      table.push(line);
      continue;
    }
    endTable();
    if (line.trim() === '') {
      endParagraph();
      continue;
    }
    if (RULE.test(line)) {
      endParagraph();
      continue;
    }
    const heading = HEADING.exec(line);
    if (heading) {
      endParagraph();
      blocks.push({ kind: 'h', inlines: parseInline(heading[1].replace(/\s#+\s*$/, '')) });
      continue;
    }
    const bullet = BULLET.exec(line);
    if (bullet) {
      endParagraph();
      blocks.push({ kind: 'li', marker: '', inlines: parseInline(bullet[1]) });
      continue;
    }
    const numbered = NUMBERED.exec(line);
    if (numbered) {
      endParagraph();
      blocks.push({ kind: 'li', marker: `${numbered[1]}.`, inlines: parseInline(numbered[2]) });
      continue;
    }
    const quote = QUOTE.exec(line);
    if (quote) {
      endParagraph();
      blocks.push({ kind: 'quote', inlines: parseInline(quote[1]) });
      continue;
    }
    // A line under a list item that is indented continues it.
    const last = blocks[blocks.length - 1];
    if (paragraph.length === 0 && last && last.kind === 'li' && /^\s{2,}\S/.test(line)) {
      last.inlines = [...last.inlines, ...parseInline(` ${line.trim()}`)];
      continue;
    }
    paragraph.push(line.trim());
  }
  endParagraph();
  endTable();
  return blocks;
}

function splitRow(row: string): string[] {
  const inner = row.trim().replace(/^\|/, '').replace(/\|$/, '');
  return inner.split('|').map(cell => cell.trim());
}

const CITE = /^\[(\d{1,4}(?:\s*[,;]\s*\d{1,4})*)\]/;
const LINK = /^\[([^\]]+)\]\(([^)\s]*)\)/;

/** Bold, italics, code and citations of one line of text. */
export function parseInline(source: string): Inline[] {
  const out: Inline[] = [];
  let strong = false;
  let em = false;
  let buffer = '';
  const flush = () => {
    if (buffer !== '') out.push({ kind: 'text', text: buffer, strong, em, code: false });
    buffer = '';
  };

  let i = 0;
  while (i < source.length) {
    const rest = source.slice(i);
    const cite = CITE.exec(rest);
    if (cite) {
      flush();
      for (const part of cite[1].split(/[,;]/)) out.push({ kind: 'cite', n: Number(part.trim()) });
      i += cite[0].length;
      continue;
    }
    const link = LINK.exec(rest);
    if (link) {
      // A link shows its words; the phone opens no addresses from an answer.
      buffer += link[1];
      i += link[0].length;
      continue;
    }
    if (rest.startsWith('`') && source.indexOf('`', i + 1) > i) {
      const end = source.indexOf('`', i + 1);
      flush();
      out.push({ kind: 'text', text: source.slice(i + 1, end), strong, em, code: true });
      i = end + 1;
      continue;
    }
    if (rest.startsWith('**') || rest.startsWith('__')) {
      const marker = rest.slice(0, 2);
      if (strong || source.indexOf(marker, i + 2) >= 0) {
        flush();
        strong = !strong;
      }
      i += 2;
      continue;
    }
    if ((rest[0] === '*' || rest[0] === '_') && !/\w/.test(source[i - 1] ?? ' ')) {
      const marker = rest[0];
      if (!em && source.indexOf(marker, i + 1) > i + 1) {
        flush();
        em = true;
        i += 1;
        continue;
      }
    }
    if (em && (rest[0] === '*' || rest[0] === '_') && !/\w/.test(source[i + 1] ?? ' ')) {
      flush();
      em = false;
      i += 1;
      continue;
    }
    buffer += rest[0];
    i += 1;
  }
  flush();
  return out;
}

/** Plain text of inline parts, citations left out (for snippets and labels). */
export function plainText(inlines: readonly Inline[]): string {
  return inlines
    .map(part => (part.kind === 'text' ? part.text : ''))
    .join('')
    .replace(/\s+/g, ' ')
    // A citation left out leaves no space before the punctuation after it.
    .replace(/\s+([.,;:!?])/g, '$1')
    .trim();
}

/** A short plain-text snippet of an answer's Markdown. */
export function snippet(markdown: string, max = 160): string {
  const text = parseAnswer(markdown)
    .map(block => (block.kind === 'code' ? '' : block.kind === 'table' ? '' : plainText(block.inlines)))
    .filter(t => t !== '')
    .join(' ');
  return text.length > max ? `${text.slice(0, max - 1).trimEnd()}…` : text;
}
