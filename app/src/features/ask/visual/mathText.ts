/**
 * Math in agent answers, rendered by remark-math + KaTeX.
 *
 * Models write math as $…$ / $$…$$ or as \(…\) / \[…\]; the latter are
 * normalised to dollars. Currency such as "$5" or "$1,200" is escaped so it is
 * not mistaken for inline math. Math segments are also protected from the
 * citation rewrite, which would otherwise turn `x[1]` into a citation pill.
 */

const MATH_TOKEN = '\x02MATH';

/** Normalise \( \) and \[ \] delimiters to $ and $$. Code is left untouched by the caller. */
export function normalizeMathDelimiters(text: string): string {
  return (
    text
      .replace(/\\\[([\s\S]+?)\\\]/g, (_, body: string) => `$$${body}$$`)
      .replace(/\\\(([\s\S]+?)\\\)/g, (_, body: string) => `$${body}$`)
      // remark-math treats `$$…$$` written on one line as inline math; an
      // equation standing on its own line is meant as display math.
      .replace(/^[ \t]*\$\$([^\n]+?)\$\$[ \t]*$/gm, (_, body: string) => `$$\n${body.trim()}\n$$`)
  );
}

/** Escape a dollar sign that starts a currency amount ("$5", "$1,200.50"). */
export function escapeCurrency(text: string): string {
  return text.replace(/(^|[^\\$])\$(?=\d)/g, '$1\\$');
}

/**
 * Replace math segments with opaque tokens; `restore` puts them back.
 * Display math ($$…$$) is matched before inline ($…$).
 */
export function protectMath(text: string): { text: string; restore: (value: string) => string } {
  const segments: string[] = [];
  const stash = (match: string) => {
    segments.push(match);
    return `${MATH_TOKEN}${segments.length - 1}\x02`;
  };
  const protectedText = text
    .replace(/\$\$[\s\S]+?\$\$/g, stash)
    .replace(/(?<![\\$])\$(?!\s)([^$\n]+?)(?<!\s)\$(?!\d)/g, stash);
  const tokenPattern = new RegExp(`${MATH_TOKEN}(\\d+)\\x02`, 'g');
  return {
    text: protectedText,
    restore: value => value.replace(tokenPattern, (_, idx: string) => segments[Number(idx)] ?? ''),
  };
}

/** Fence languages drawn as Mermaid diagrams. */
const MERMAID_LANGS = new Set([
  'mermaid', 'flowchart', 'graph', 'sequence', 'sequencediagram', 'classdiagram', 'statediagram',
  'erdiagram', 'mindmap', 'timeline', 'gantt', 'journey', 'gitgraph', 'quadrantchart',
]);

const MERMAID_HEADER = /^\s*(graph|flowchart|sequenceDiagram|classDiagram|stateDiagram(-v2)?|erDiagram|journey|gantt|pie|gitGraph|mindmap|timeline|quadrantChart|xychart-beta|block-beta|sankey-beta)\b/;

export function isMermaidLanguage(lang: string): boolean {
  return MERMAID_LANGS.has(lang.toLowerCase());
}

/** Mermaid source for a fence; a bare ```flowchart body gets its header added. */
export function mermaidSource(lang: string, body: string): string {
  if (MERMAID_HEADER.test(body)) return body;
  const l = lang.toLowerCase();
  if (l === 'sequence' || l === 'sequencediagram') return `sequenceDiagram\n${body}`;
  if (l === 'classdiagram') return `classDiagram\n${body}`;
  if (l === 'statediagram') return `stateDiagram-v2\n${body}`;
  if (l === 'erdiagram') return `erDiagram\n${body}`;
  if (l === 'mindmap') return `mindmap\n${body}`;
  if (l === 'timeline') return `timeline\n${body}`;
  return `flowchart TD\n${body}`;
}
