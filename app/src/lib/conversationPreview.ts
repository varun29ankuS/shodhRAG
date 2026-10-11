/**
 * What a conversation is about, derived from its saved messages (no model
 * call): the opening question, the gist of the latest answer, the sources it
 * used, and its size. Shown when hovering or focusing a chat in the sidebar.
 */

export interface PreviewMessage {
  role: string;
  content: string;
  searchResults?: unknown[];
  transcript?: Record<string, unknown>;
}

export interface ConversationPreview {
  firstQuestion: string | null;
  latestAnswer: string | null;
  sources: string[];
  questionCount: number;
}

const QUESTION_CHARS = 180;
const ANSWER_CHARS = 240;
const MAX_SOURCES = 3;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/**
 * Markdown/LaTeX to a readable one-paragraph string: diagrams, code and
 * follow-up blocks are dropped, display math becomes "[equation]", inline
 * math keeps its symbols, citation markers and markdown syntax are removed.
 */
export function plainText(markdown: string): string {
  return markdown
    .replace(/```(?:mermaid|chart|followups|flowchart)[\s\S]*?```/gi, ' ')
    .replace(/```[\s\S]*?```/g, ' [code] ')
    .replace(/\$\$[\s\S]+?\$\$/g, ' [equation] ')
    .replace(/\\\[[\s\S]+?\\\]/g, ' [equation] ')
    .replace(/\\\(([\s\S]+?)\\\)/g, '$1')
    .replace(/\$([^$\n]+)\$/g, '$1')
    .replace(/\\(mathbf|mathrm|text|operatorname)\{([^}]*)\}/g, '$2')
    .replace(/\\([a-zA-Z]+)/g, '$1')
    .replace(/[{}]/g, '')
    .replace(/\[(?:Document\s+)?\d+(?:\s*,\s*\d+)*\]/g, '')
    .replace(/【\d+†[^】]*】/g, '')
    .replace(/!\[[^\]]*\]\([^)]*\)/g, '')
    .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
    .replace(/^\s{0,3}(#{1,6}|>|[-*+]|\d+\.)\s+/gm, '')
    .replace(/(\*\*|\*|`)/g, '')
    // Underscore emphasis only around whole words, so math like phi_q survives.
    .replace(/(^|\s)__?([^_\s][^_]*?)__?(?=\s|$|[.,;:!?])/g, '$1$2')
    .replace(/\|/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
}

/** Cut at a word boundary with an ellipsis. */
export function clip(text: string, max: number): string {
  const chars = Array.from(text);
  if (chars.length <= max) return text;
  const cut = chars.slice(0, max).join('');
  const space = cut.lastIndexOf(' ');
  return `${(space > max * 0.6 ? cut.slice(0, space) : cut).trimEnd()}…`;
}

function fileLabel(path: string): string {
  const calendar = /^calendar:\/\/(task|event)\//i.exec(path);
  if (calendar) return calendar[1].toLowerCase() === 'task' ? 'Tasks' : 'Calendar';
  if (/^note:\/\//i.test(path)) return 'Notes';
  if (/^https?:\/\//i.test(path)) {
    try {
      return new URL(path).hostname.replace(/^www\./, '');
    } catch {
      return path;
    }
  }
  const name = path.split(/[/\\]/).pop() || path;
  const dot = name.lastIndexOf('.');
  return dot > 0 ? name.slice(0, dot) : name;
}

/** Source paths an answer drew on: legacy search results and agent passages. */
function answerSources(message: PreviewMessage): string[] {
  const paths: string[] = [];
  for (const r of message.searchResults ?? []) {
    if (!isRecord(r)) continue;
    const citation = isRecord(r.citation) ? r.citation : null;
    const p = r.sourceFile ?? r.source_file ?? citation?.source;
    if (typeof p === 'string' && p) paths.push(p);
  }
  const passages = message.transcript && Array.isArray(message.transcript.passages) ? message.transcript.passages : [];
  for (const p of passages) {
    if (!isRecord(p)) continue;
    const path = typeof p.path === 'string' && p.path ? p.path : typeof p.file === 'string' ? p.file : '';
    if (path) paths.push(path);
  }
  return paths;
}

export function conversationPreview(messages: readonly PreviewMessage[]): ConversationPreview {
  const questions = messages.filter(m => m.role === 'user' && m.content.trim());
  const firstQuestion = questions.length > 0 ? clip(plainText(questions[0].content), QUESTION_CHARS) : null;

  let latestAnswer: string | null = null;
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i];
    if (m.role !== 'assistant') continue;
    const text = plainText(m.content);
    if (text) {
      latestAnswer = clip(text, ANSWER_CHARS);
      break;
    }
  }

  // Most-cited sources first, by how many answers used them.
  const counts = new Map<string, number>();
  for (const m of messages) {
    if (m.role !== 'assistant') continue;
    for (const label of new Set(answerSources(m).map(fileLabel))) {
      counts.set(label, (counts.get(label) ?? 0) + 1);
    }
  }
  const sources = [...counts.entries()]
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .slice(0, MAX_SOURCES)
    .map(([label]) => label);

  return { firstQuestion, latestAnswer, sources, questionCount: questions.length };
}
