/**
 * Deterministic repair of mermaid source that the parser rejected.
 *
 * Weaker models write diagrams that are almost right: labels with
 * parentheses or quotes left unquoted, unicode arrows, HTML formatting,
 * subgraph titles with punctuation, an edge with no target, the header
 * repeated. Each rule below fixes one such mistake and only touches text
 * that cannot parse as written, so a valid diagram comes back unchanged
 * (HTML tags other than <br> are the one exception: they are removed
 * wherever they appear). Structural rules (labels, edges, subgraphs) run
 * on flowcharts only; other diagram kinds get the few fixes that are safe
 * for their grammar.
 *
 * Applying the repair twice gives the same result as applying it once.
 *
 * Pure module (no React, no mermaid), unit-tested with Node
 * (`app/tests/mermaidRepair.test.ts`).
 */

export type MermaidRepairFix =
  | 'quoted-labels'
  | 'arrows'
  | 'html-tags'
  | 'semicolons'
  | 'subgraph-titles'
  | 'empty-edges'
  | 'duplicate-headers';

export interface MermaidRepairResult {
  /** The repaired source, or the input itself when nothing changed. */
  source: string;
  changed: boolean;
  /** Which kinds of fixes were applied, in a stable order. */
  fixes: MermaidRepairFix[];
}

type DiagramKind = 'flowchart' | 'sequence' | 'class' | 'er' | 'pie' | 'gantt' | 'journey' | 'other';

const FIX_ORDER: readonly MermaidRepairFix[] = [
  'duplicate-headers', 'html-tags', 'arrows', 'quoted-labels', 'subgraph-titles', 'empty-edges', 'semicolons',
];

/** Diagram kinds whose grammar has no `;` separator (a trailing one is rejected). */
const NO_SEMICOLON_KINDS: ReadonlySet<DiagramKind> = new Set(['class', 'er', 'pie', 'gantt', 'journey']);

/** Characters that end an unquoted flowchart label early (found against the mermaid 11 parser). */
const LABEL_BREAKERS = /[()[\]{}"|]/;
/** Characters a bare subgraph title cannot hold. */
const SUBGRAPH_BREAKERS = /[()[\]{},"<>|=@~→⟶⇒⟹➔➜]/;

/**
 * HTML formatting tags models put in labels. `br` is kept (mermaid draws it
 * as a line break); generic-type brackets such as `List<String>` are not
 * tags in this list and are left alone.
 */
const HTML_TAG = new RegExp(
  '<\\/?(?:b|i|u|s|em|strong|span|div|p|font|small|big|sub|sup|code|mark|a|center|strike|del|ins|h[1-6]|ul|ol|li|tt|kbd|var|abbr|cite|q|label|img|hr|table|thead|tbody|tr|td|th)(?:\\s[^<>]*)?\\/?>',
  'gi',
);

const HEADER = /^(graph|flowchart|sequenceDiagram|classDiagram(?:-v2)?|stateDiagram(?:-v2)?|erDiagram|journey|gantt|pie|gitGraph|mindmap|timeline|quadrantChart|requirementDiagram|xychart-beta|block-beta|sankey-beta|packet-beta|architecture-beta|kanban|C4Context|C4Container|C4Component|C4Dynamic|C4Deployment)\b/;

/** A line that only restates a flowchart header (`graph TD`, `flowchart LR;`). */
const FLOW_HEADER_ONLY = /^(?:graph|flowchart)(?:\s+(?:TB|TD|BT|RL|LR))?\s*;?$/i;

/** Flowchart lines that are not node/edge statements. */
const FLOW_KEYWORD_LINE = /^(?:end|classDef|class|style|linkStyle|click|accTitle|accDescr|direction)\b/;

const ID_CHAR = /[\p{L}\p{N}_]/u;

interface Shape {
  open: string;
  close: readonly string[];
}

/** Flowchart node shapes, longest opener first. */
const SHAPES: readonly Shape[] = [
  { open: '(((', close: [')))'] },
  { open: '((', close: ['))'] },
  { open: '([', close: ['])'] },
  { open: '[[', close: [']]'] },
  { open: '[(', close: [')]'] },
  { open: '[/', close: ['/]', '\\]'] },
  { open: '[\\', close: ['\\]', '/]'] },
  { open: '{{', close: ['}}'] },
  { open: '[', close: [']'] },
  { open: '(', close: [')'] },
  { open: '{', close: ['}'] },
  { open: '>', close: [']'] },
];

/** What may follow a node's closing bracket: the end, a separator, a class, `&` or a link (unicode arrows included). */
const AFTER_NODE = /^\s*(?:$|;|:::|&|<?(?:-{2,}|={2,}|-\.|~~~|->)|[→⟶➔➜➝➞⇢⇒⟹⇨↔⟷⇔—–])/;

/** The end of a `-- text -->` link: the closing arrow after the text. */
const LINK_TEXT_END = /\s+(?:<?-{2,}>|-{3,}|={2,}>|={3,}|\.-+>?|-{2,}[ox])(?=\s|$|[\p{L}\p{N}_"])/u;

/** A link with nothing after it, at the end of a statement. */
const TRAILING_LINK = /\s*(?:<?(?:-{2,}>?|={2,}>?|-\.+->?)|~~~)\s*(?:\|[^|]*\|)?\s*;?\s*$/;
/** A link with nothing before it, at the start of a statement. */
const LEADING_LINK = /^(\s*)(?:<?(?:-{2,}>?|={2,}>?|-\.+->?)|~~~)\s*(?:\|[^|]*\|)?\s*/;

/** The kind of diagram from its header line. */
function diagramKind(header: string): DiagramKind {
  const keyword = HEADER.exec(header)?.[1].toLowerCase() ?? '';
  if (keyword === 'graph' || keyword === 'flowchart') return 'flowchart';
  if (keyword === 'sequencediagram') return 'sequence';
  if (keyword.startsWith('classdiagram')) return 'class';
  if (keyword === 'erdiagram') return 'er';
  if (keyword === 'pie') return 'pie';
  if (keyword === 'gantt') return 'gantt';
  if (keyword === 'journey') return 'journey';
  return 'other';
}

/** Escapes inner double quotes the way mermaid labels spell them. */
function escapeQuotes(text: string): string {
  return text.replace(/"/g, '#quot;');
}

/**
 * A label made parseable: wrapped in double quotes when it holds a
 * character that ends an unquoted label. Labels already quoted are kept;
 * a quoted label with stray quotes inside has them escaped.
 */
export function quoteLabel(label: string): string {
  const trimmed = label.trim();
  if (trimmed.length >= 2 && trimmed.startsWith('"') && trimmed.endsWith('"')) {
    const inner = trimmed.slice(1, -1);
    if (!inner.includes('"')) return label;
    return `"${escapeQuotes(inner)}"`;
  }
  if (!LABEL_BREAKERS.test(label)) return label;
  return `"${escapeQuotes(label)}"`;
}

/** Unicode arrows and dashes between flowchart nodes as mermaid links. */
function normalizeFlowArrows(text: string): string {
  return text
    .replace(/[⇒⟹⇨]/g, '==>')
    .replace(/[↔⟷⇔]/g, '<-->')
    .replace(/[-—–]*[—–][-—–]*>/g, '-->')
    .replace(/[→⟶➔➜➝➞⇢]/g, '-->')
    .replace(/(^|\s)[—–]{1,2}(?=\s)/g, '$1---')
    .replace(/(^|[^-.=<>])->(?!>)/g, '$1-->');
}

interface Located {
  content: string;
  close: string;
  end: number;
}

/**
 * The label of a node shape opening at `start`: the first closing bracket
 * that is followed by what may follow a node (so brackets inside an
 * unquoted label do not end it), else the first closing bracket.
 */
function locateLabel(line: string, start: number, shape: Shape): Located | null {
  if (line[start] === '"') {
    const q = line.indexOf('"', start + 1);
    if (q > 0) {
      for (const close of shape.close) {
        if (line.startsWith(close, q + 1)) return { content: line.slice(start, q + 1), close, end: q + 1 + close.length };
      }
    }
  }
  let fallback: Located | null = null;
  for (let p = start; p < line.length; p++) {
    for (const close of shape.close) {
      if (!line.startsWith(close, p)) continue;
      const found = { content: line.slice(start, p), close, end: p + close.length };
      if (AFTER_NODE.test(line.slice(found.end))) return found;
      fallback ??= found;
    }
  }
  return fallback;
}

/** Index after a `@{ … }` shape block opening at `start` (quotes respected), or -1. */
function shapeBlockEnd(line: string, start: number): number {
  let depth = 0;
  let quoted = false;
  for (let p = start; p < line.length; p++) {
    const ch = line[p];
    if (ch === '"') quoted = !quoted;
    if (quoted) continue;
    if (ch === '{') depth += 1;
    if (ch === '}') {
      depth -= 1;
      if (depth === 0) return p + 1;
    }
  }
  return -1;
}

/**
 * One flowchart statement line: node labels, `|edge labels|` and
 * `-- link text -->` quoted where needed; unicode arrows between nodes
 * (never inside labels) normalised.
 */
function repairFlowStatement(line: string, fixes: Set<MermaidRepairFix>): string {
  let out = '';
  let plain = '';
  const flush = () => {
    const arrows = normalizeFlowArrows(plain);
    if (arrows !== plain) fixes.add('arrows');
    out += arrows;
    plain = '';
  };
  const quote = (label: string) => {
    const quoted = quoteLabel(label);
    if (quoted !== label) fixes.add('quoted-labels');
    return quoted;
  };
  let i = 0;
  while (i < line.length) {
    const ch = line[i];
    if (ch === '"') {
      const end = line.indexOf('"', i + 1);
      if (end < 0) {
        plain += line.slice(i);
        break;
      }
      flush();
      out += line.slice(i, end + 1);
      i = end + 1;
      continue;
    }
    if (ch === '|') {
      const end = line.indexOf('|', i + 1);
      if (end < 0) {
        plain += ch;
        i += 1;
        continue;
      }
      flush();
      out += `|${quote(line.slice(i + 1, end))}|`;
      i = end + 1;
      continue;
    }
    const prev = i > 0 ? line[i - 1] : '';
    const open = /^(--|==|-\.)(\s+)/.exec(line.slice(i));
    if (open && !/[-=<.]/.test(prev)) {
      const textStart = i + open[0].length;
      const rest = line.slice(textStart);
      const close = LINK_TEXT_END.exec(rest);
      if (close && close.index > 0 && rest[0] !== '|') {
        flush();
        out += open[0] + quote(rest.slice(0, close.index));
        i = textStart + close.index;
        continue;
      }
    }
    if (ID_CHAR.test(ch) && !(prev && ID_CHAR.test(prev))) {
      let j = i;
      while (j < line.length && ID_CHAR.test(line[j])) j += 1;
      plain += line.slice(i, j);
      i = j;
      if (line.startsWith('@{', i)) {
        const end = shapeBlockEnd(line, i + 1);
        if (end > 0) {
          flush();
          out += line.slice(i, end);
          i = end;
        }
        continue;
      }
      const shape = SHAPES.find(s => line.startsWith(s.open, i));
      if (!shape) continue;
      const label = locateLabel(line, i + shape.open.length, shape);
      if (!label) continue;
      flush();
      out += shape.open + quote(label.content) + label.close;
      i = label.end;
      continue;
    }
    plain += ch;
    i += 1;
  }
  flush();
  return out;
}

/** A link with no node on one side removed; '' when nothing is left. */
function dropEmptyLinks(line: string): string {
  let next = line;
  // Repeated: `A --> B --> -->` loses both dangling links.
  for (;;) {
    const trimmed = next.replace(TRAILING_LINK, '');
    if (trimmed === next) break;
    next = trimmed;
  }
  next = next.replace(LEADING_LINK, '$1');
  return next.trim() ? next : '';
}

/** `subgraph` with a title that cannot stand bare quoted. */
function repairSubgraph(line: string): string {
  const m = /^(\s*subgraph)(\s+)(.*?)(\s*;?\s*)$/.exec(line);
  if (!m) return line;
  const [, keyword, gap, rest, tail] = m;
  if (!rest || /^"[^"]*"$/.test(rest)) return line;
  const titled = /^([^\s[\]"]+)(\s*)\[(.*)\]$/.exec(rest);
  if (titled) return `${keyword}${gap}${titled[1]}${titled[2]}[${quoteLabel(titled[3])}]${tail}`;
  if (!SUBGRAPH_BREAKERS.test(rest)) return line;
  const inner = rest.startsWith('"') && rest.endsWith('"') && rest.length >= 2 ? rest.slice(1, -1) : rest;
  return `${keyword}${gap}"${escapeQuotes(inner)}"${tail}`;
}

/** A sequence message: unicode arrows as `->>`, `;` inside the text escaped. */
function repairSequenceLine(line: string, fixes: Set<MermaidRepairFix>): string {
  const colon = line.indexOf(':');
  const head = colon >= 0 ? line.slice(0, colon) : line;
  const tail = colon >= 0 ? line.slice(colon) : '';
  const fixedHead = head.replace(/\s*[→⟶➔➜⇒⟹]\s*/g, '->>');
  if (fixedHead !== head) fixes.add('arrows');
  // A `;` ends the statement: inside the text it cuts the message short.
  // Entities (`#59;`, `#quot;`) end in `;` too and are left as they are.
  const fixedTail = tail.replace(/(?<!#[A-Za-z0-9]+);(?=\s*\S)/g, '#59;');
  if (fixedTail !== tail) fixes.add('semicolons');
  return fixedHead + fixedTail;
}

/** Index of the header line, skipping front matter, directives and comments. */
function headerIndex(lines: readonly string[]): { index: number; bodyStart: number } {
  let i = 0;
  while (i < lines.length && !lines[i].trim()) i += 1;
  if (i < lines.length && lines[i].trim() === '---') {
    let j = i + 1;
    while (j < lines.length && lines[j].trim() !== '---') j += 1;
    i = j + 1;
  }
  while (i < lines.length) {
    const t = lines[i].trim();
    if (t && !t.startsWith('%%')) break;
    i += 1;
  }
  return { index: i, bodyStart: i + 1 };
}

/**
 * Repairs `source` for the mermaid parser. Returns the input unchanged
 * (`changed: false`) when no rule applies.
 */
export function repairMermaid(source: string): MermaidRepairResult {
  const normalized = source.replace(/\r\n?/g, '\n');
  const lines = normalized.split('\n');
  const { index, bodyStart } = headerIndex(lines);
  if (index >= lines.length) return { source, changed: false, fixes: [] };
  const header = lines[index].trim();
  const kind = diagramKind(header);
  const headerKeyword = HEADER.exec(header)?.[1].toLowerCase() ?? '';
  const fixes = new Set<MermaidRepairFix>();
  const note = (fix: MermaidRepairFix, before: string, after: string) => {
    if (before !== after) fixes.add(fix);
  };

  const out: string[] = lines.slice(0, bodyStart);
  // Direction lines seen per open block (top level first, then each subgraph).
  const directions: boolean[] = [false];

  for (let n = bodyStart; n < lines.length; n++) {
    let line = lines[n];
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith('%%')) {
      out.push(line);
      continue;
    }

    // A second header: models restate `graph TD` (or the diagram keyword) mid-diagram.
    if (kind === 'flowchart' ? FLOW_HEADER_ONLY.test(trimmed) : trimmed.replace(/;$/, '').toLowerCase() === headerKeyword && headerKeyword !== '') {
      fixes.add('duplicate-headers');
      continue;
    }

    const untagged = line.replace(HTML_TAG, '');
    note('html-tags', line, untagged);
    line = untagged;

    if (kind === 'flowchart') {
      const t = line.trim();
      if (/^subgraph\b/.test(t)) {
        directions.push(false);
        const fixed = repairSubgraph(line);
        note('subgraph-titles', line, fixed);
        out.push(fixed);
        continue;
      }
      if (/^end\b/.test(t)) {
        if (directions.length > 1) directions.pop();
        out.push(line);
        continue;
      }
      if (/^direction\b/.test(t)) {
        if (directions[directions.length - 1]) {
          fixes.add('duplicate-headers');
          continue;
        }
        directions[directions.length - 1] = true;
        out.push(line);
        continue;
      }
      if (FLOW_KEYWORD_LINE.test(t)) {
        out.push(line);
        continue;
      }
      const statement = repairFlowStatement(line, fixes);
      const linked = dropEmptyLinks(statement);
      if (linked !== statement) fixes.add('empty-edges');
      if (linked) out.push(linked);
      continue;
    }

    if (kind === 'sequence') {
      out.push(repairSequenceLine(line, fixes));
      continue;
    }

    if (NO_SEMICOLON_KINDS.has(kind)) {
      // These grammars have no `;` separator: a trailing one is a syntax error.
      const fixed = line.replace(/\s*;+\s*$/, '');
      note('semicolons', line, fixed);
      out.push(fixed);
      continue;
    }

    out.push(line);
  }

  const repaired = out.join('\n');
  if (repaired === normalized) return { source, changed: false, fixes: [] };
  return { source: repaired, changed: true, fixes: FIX_ORDER.filter(f => fixes.has(f)) };
}
