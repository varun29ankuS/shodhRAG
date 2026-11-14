/**
 * A scripted computer for CI's screenshots of the connected screens. It is
 * only in a debug build: main.tsx imports it behind `__SHODH_DEMO__`, which
 * vite.config.ts sets from the Tauri CLI's `TAURI_ENV_DEBUG`, so a release
 * build's JavaScript does not contain it; and only a debug build's Rust side
 * reads the setting that opens it (`start_screen`). The screens render it
 * through the same link transport as a real computer's answers.
 *
 * Everything here is sample data written for the screenshots, not a user's.
 * CI checks a release build's JavaScript has none of it (its refusal text,
 * "The sample computer has no").
 */
import type { LinkSource, LinkStatus } from './link';
import { initialNav, type Nav } from './nav';

const ROOT = '/Users/sample/Papers';
const RETRIEVAL = `${ROOT}/Retrieval`;
const SELF_RAG = `${RETRIEVAL}/self-rag.pdf`;
const MIDDLE = `${RETRIEVAL}/lost-in-the-middle.pdf`;
const RAPTOR = `${ROOT}/raptor.pdf`;
const NOTES = `${ROOT}/reading-notes.md`;

const minutesAgo = (m: number) => new Date(Date.now() - m * 60_000).toISOString();

const PASSAGES = [
  { n: 1, file: 'self-rag.pdf', path: SELF_RAG, page: '3', heading: 'Self-RAG', text: 'Before generating each segment, the model emits a Retrieve token. Passages are fetched only when that token indicates retrieval is needed.', web: false },
  { n: 2, file: 'self-rag.pdf', path: SELF_RAG, page: '4', heading: 'Critique tokens', text: 'Each retrieved passage is judged with critique tokens: whether it is relevant, whether it supports the output, and whether the output is useful.', web: false },
  { n: 3, file: 'lost-in-the-middle.pdf', path: MIDDLE, page: '6', heading: 'Position of relevant information', text: 'Accuracy is highest when the relevant document is at the beginning or end of the input.', web: false },
];

const ANSWER = [
  'It lets the model decide for itself, instead of retrieving on every question [1].',
  '',
  '## How it works',
  '',
  '- **Retrieve token.** Before each part of the answer, the model predicts whether it needs evidence and fetches passages only when it does [1].',
  '- **Critique tokens.** It grades each passage: is it relevant, does it support the claim, is the answer useful [2].',
  '- **Adjustable later.** These are ordinary tokens, so retrieval can be made more or less frequent without retraining [2].',
  '',
  'Where the passages go matters too: models use the start and the end of a long context best [3].',
].join('\n');

const CONVERSATIONS = [
  { id: 'demo-selfrag', title: 'How does Self-RAG decide when to retrieve?', updatedAt: minutesAgo(4), pinned: false, messageCount: 2, workspaceId: 'w1', mode: null, agentMode: null },
  { id: 'demo-rerank', title: 'Make the reranker skip empty passages', updatedAt: minutesAgo(48), pinned: false, messageCount: 2, workspaceId: 'w1', mode: 'code', agentMode: 'ask' },
  { id: 'demo-middle', title: 'Where should the best passages go in the prompt?', updatedAt: minutesAgo(130), pinned: false, messageCount: 4, workspaceId: 'w1', mode: 'research', agentMode: null },
  { id: 'demo-notes', title: 'What do my reading notes say about retrieval?', updatedAt: minutesAgo(60 * 26), pinned: true, messageCount: 2, workspaceId: null, mode: null, agentMode: null },
  { id: 'demo-poster', title: 'A one-page poster of the chapter 2 results', updatedAt: minutesAgo(60 * 27), pinned: false, messageCount: 2, workspaceId: 'w2', mode: 'code', agentMode: 'creator' },
  { id: 'demo-hnsw', title: 'Compare HNSW and IVF for my index size', updatedAt: minutesAgo(60 * 24 * 3), pinned: false, messageCount: 6, workspaceId: 'w1', mode: null, agentMode: null },
  { id: 'demo-bench', title: 'Add a benchmark for the int8 index', updatedAt: minutesAgo(60 * 24 * 9), pinned: false, messageCount: 4, workspaceId: 'w2', mode: 'code', agentMode: 'acceptEdits' },
].map(c => ({ ...c, createdAt: c.updatedAt }));

const CODE_ANSWER = [
  'The reranker now skips passages whose text is empty or only whitespace, before scoring, so they never reach the prompt.',
  '',
  'I added a test with two empty passages and one real one; the suite passes.',
].join('\n');

/** A Code answer's saved transcript: two files changed, two commands run. */
const CODE_TRANSCRIPT = {
  runId: 'demo-rerank-run',
  startedAtMs: 0,
  status: 'completed',
  agentMode: 'ask',
  blocks: [
    { kind: 'step', stepId: 'read-1' },
    { kind: 'step', stepId: 'edit-1' },
    { kind: 'step', stepId: 'write-1' },
    { kind: 'step', stepId: 'cmd-1' },
    { kind: 'step', stepId: 'cmd-2' },
    { kind: 'text', id: 'm1', text: CODE_ANSWER },
  ],
  steps: {
    'read-1': { id: 'read-1', tool: 'read', label: 'Read rerank.rs', status: 'done', args: { path: 'crates/shodh-rag/src/rerank.rs' }, startedAtMs: 1 },
    'edit-1': { id: 'edit-1', tool: 'edit', label: 'Edit rerank.rs', status: 'done', args: { path: 'crates/shodh-rag/src/rerank.rs' }, startedAtMs: 2 },
    'write-1': { id: 'write-1', tool: 'write', label: 'Write rerank_empty.rs', status: 'done', args: { path: 'crates/shodh-rag/tests/rerank_empty.rs' }, startedAtMs: 3 },
    'cmd-1': {
      id: 'cmd-1',
      tool: 'run_command',
      label: 'Run cargo test',
      status: 'done',
      args: { command: 'cargo test -p shodh-rag --test rerank_empty' },
      startedAtMs: 4,
      command: {
        commandId: 'c1',
        command: 'cargo test -p shodh-rag --test rerank_empty',
        state: 'done',
        startedAtMs: 4,
        timeLimitSecs: 600,
        sandbox: null,
        exitCode: 0,
        note: null,
        durationMs: 18_400,
        output: 'running 3 tests\ntest empty_passages_are_skipped ... ok\ntest whitespace_is_empty ... ok\ntest real_passages_are_kept ... ok\n\ntest result: ok. 3 passed; 0 failed',
        outputCut: false,
      },
    },
    'cmd-2': {
      id: 'cmd-2',
      tool: 'run_command',
      label: 'Run cargo clippy',
      status: 'done',
      args: { command: 'cargo clippy -p shodh-rag -- -D warnings' },
      startedAtMs: 5,
      command: {
        commandId: 'c2',
        command: 'cargo clippy -p shodh-rag -- -D warnings',
        state: 'done',
        startedAtMs: 5,
        timeLimitSecs: 600,
        sandbox: null,
        exitCode: 0,
        note: null,
        durationMs: 9_100,
        output: '    Checking shodh-rag v0.0.10\n    Finished `dev` profile in 9.06s',
        outputCut: false,
      },
    },
  },
  passages: [],
  groundings: [],
  interimIds: [],
};

const CREATOR_ANSWER = 'The poster is ready: one page with the three results and the recall chart.';

/** A Creator answer's saved transcript: what it made, and its finish summary. */
const CREATOR_TRANSCRIPT = {
  runId: 'demo-poster-run',
  startedAtMs: 0,
  status: 'completed',
  agentMode: 'creator',
  blocks: [{ kind: 'text', id: 'm1', text: CREATOR_ANSWER }],
  steps: {
    'write-1': { id: 'write-1', tool: 'write', label: 'Write index.html', status: 'done', args: { path: 'index.html' }, startedAtMs: 1 },
    'write-2': { id: 'write-2', tool: 'write', label: 'Write poster.css', status: 'done', args: { path: 'poster.css' }, startedAtMs: 2 },
    'finish-1': { id: 'finish-1', tool: 'finish_creation', label: 'Finish', status: 'done', args: {}, startedAtMs: 3, detail: { summary: 'A one-page poster of the chapter 2 results, with the recall chart.', choices: [] } },
  },
  passages: [],
  groundings: [],
  interimIds: [],
};

function conversation(id: string) {
  const summary = CONVERSATIONS.find(c => c.id === id);
  if (!summary) return null;
  let content = ANSWER;
  let transcript: unknown = {
    runId: `${id}-run`,
    startedAtMs: 0,
    status: 'completed',
    blocks: [{ kind: 'text', id: 'm1', text: ANSWER }],
    steps: {},
    passages: PASSAGES,
    groundings: [],
    interimIds: [],
  };
  if (summary.mode === 'code' && summary.agentMode !== 'creator') {
    content = CODE_ANSWER;
    transcript = CODE_TRANSCRIPT;
  } else if (summary.agentMode === 'creator') {
    content = CREATOR_ANSWER;
    transcript = CREATOR_TRANSCRIPT;
  }
  return {
    id,
    title: summary.title,
    updatedAt: summary.updatedAt,
    workspaceId: summary.workspaceId,
    mode: summary.mode,
    agentMode: summary.agentMode,
    messages: [
      { id: `${id}-q`, role: 'user', content: summary.title, timestamp: summary.updatedAt },
      { id: `${id}-a`, role: 'assistant', content, timestamp: summary.updatedAt, transcript },
    ],
  };
}

const FOLDERS: Record<string, { folders: Array<{ name: string; path: string }>; files: Array<{ name: string; path: string }> }> = {
  [ROOT]: {
    folders: [
      { name: 'Indexing', path: `${ROOT}/Indexing` },
      { name: 'Retrieval', path: RETRIEVAL },
    ],
    files: [
      { name: 'raptor.pdf', path: RAPTOR },
      { name: 'reading-notes.md', path: NOTES },
    ],
  },
  [RETRIEVAL]: {
    folders: [],
    files: [
      { name: 'lost-in-the-middle.pdf', path: MIDDLE },
      { name: 'self-rag.pdf', path: SELF_RAG },
    ],
  },
  [`${ROOT}/Indexing`]: { folders: [], files: [] },
};

const TITLES: Record<string, string> = {
  [SELF_RAG]: 'Self-RAG: Learning to Retrieve, Generate, and Critique through Self-Reflection',
  [MIDDLE]: 'Lost in the Middle: How Language Models Use Long Contexts',
  [RAPTOR]: 'RAPTOR: Recursive Abstractive Processing for Tree-Organized Retrieval',
};

/** A drawn page: a title, two columns of lines, a figure, and the cited line marked. */
function drawPage(path: string, page: number, width: number): { png: string; width: number; height: number } {
  const height = Math.round(width * 1.294);
  const canvas = document.createElement('canvas');
  canvas.width = width;
  canvas.height = height;
  const g = canvas.getContext('2d');
  if (!g) return { png: '', width, height };
  const u = width / 320;
  g.fillStyle = '#fbfaf7';
  g.fillRect(0, 0, width, height);
  let y = 26 * u;
  if (page === 1) {
    g.fillStyle = '#1c1c1f';
    g.font = `600 ${15 * u}px Georgia, serif`;
    const words = (TITLES[path] ?? path.split('/').pop() ?? '').split(' ');
    let line = '';
    for (const word of words) {
      const next = line ? `${line} ${word}` : word;
      if (g.measureText(next).width > width - 48 * u) {
        g.fillText(line, 24 * u, y);
        y += 19 * u;
        line = word;
      } else line = next;
    }
    g.fillText(line, 24 * u, y);
    y += 16 * u;
    g.fillStyle = '#9b988f';
    g.fillRect(24 * u, y, 120 * u, 3 * u);
    y += 18 * u;
  }
  const column = (x: number, top: number, w: number, rows: number, mark: number) => {
    for (let i = 0; i < rows; i++) {
      if (i === mark) {
        g.fillStyle = 'rgba(232, 72, 42, 0.28)';
        g.fillRect(x - 2 * u, top + i * 9 * u - 3 * u, w + 4 * u, 22 * u);
      }
      g.fillStyle = '#cfccc6';
      g.fillRect(x, top + i * 9 * u, w * (i % 7 === 6 ? 0.6 : 0.92 + (i % 3) * 0.03), 2.5 * u);
    }
  };
  const colWidth = (width - 64 * u) / 2;
  column(24 * u, y, colWidth, 26, page === 3 || page === 4 || page === 6 ? 8 : -1);
  g.strokeStyle = '#cfccc6';
  g.lineWidth = u;
  g.strokeRect(40 * u + colWidth, y, colWidth, 70 * u);
  const bars = [0.4, 0.75, 0.55, 0.9, 0.65];
  bars.forEach((b, i) => {
    g.fillStyle = i === 2 ? '#e8a08a' : '#9fb4d9';
    g.fillRect(48 * u + colWidth + i * 16 * u, y + 64 * u - 56 * b * u, 9 * u, 56 * b * u);
  });
  column(40 * u + colWidth, y + 84 * u, colWidth, 17, -1);
  return { png: canvas.toDataURL('image/png').replace(/^data:image\/png;base64,/, ''), width, height };
}

const extension = (path: string) => path.split('.').pop()?.toLowerCase() ?? '';

/** The scripted computer, connected. */
export function demoLink(): LinkSource {
  const handlers = new Set<(name: string, payload: unknown) => void>();
  const send = (name: string, payload: unknown) => {
    for (const handler of [...handlers]) handler(name, payload);
  };
  const status: LinkStatus = { state: 'connected' };

  const call = async (command: string, args: Record<string, unknown>): Promise<unknown> => {
    switch (command) {
      case 'conversations_list':
        return CONVERSATIONS;
      case 'load_conversation':
        return conversation(String(args.conversationId));
      case 'library_folders':
        return args.path === undefined ? { folders: [{ name: 'Papers', path: ROOT }, { name: 'Notes', path: '/Users/sample/Notes' }] } : (FOLDERS[String(args.path)] ?? { folders: [], files: [] });
      case 'get_source_file_info': {
        const path = String(args.filePath);
        const ext = extension(path);
        return { path, fileName: path.split('/').pop(), extension: ext, sizeBytes: 1_000_000, kind: ext === 'pdf' ? 'pdf' : ext === 'md' ? 'text' : 'unsupported' };
      }
      case 'read_source_page': {
        const page = Number(args.page);
        const drawn = drawPage(String(args.filePath), page, Math.min(1600, Math.max(320, Number(args.width))));
        return { page, pages: 18, ...drawn };
      }
      case 'read_source_text':
        return { text: '# Reading notes: when to retrieve\n\nRetrieve less, but better. Order matters: key passages belong at the edges of the context.', part: 0, parts: 1, truncated: false };
      case 'approvals_pending':
        return [
          { sessionId: 'demo-s1', stepId: 'note-1', conversationId: 'demo-selfrag', tool: 'create_note', title: 'Save a note about when Self-RAG retrieves', requestedAt: minutesAgo(2), phoneMay: true },
          { sessionId: 'demo-s2', stepId: 'cmd-1', conversationId: null, tool: 'run_command', title: 'Run the test suite in shodh-rag', requestedAt: minutesAgo(9), phoneMay: false },
        ];
      case 'workspaces_list':
        return [
          { id: 'w1', name: 'Vector DB reading', description: '', color: '', pinned: true, chatCount: 3, lastActiveAt: minutesAgo(4) },
          { id: 'w2', name: 'Thesis, chapter 2', description: '', color: '', pinned: false, chatCount: 9, lastActiveAt: minutesAgo(60 * 30) },
          { id: 'w3', name: 'Grant proposal', description: '', color: '', pinned: false, chatCount: 2, lastActiveAt: null },
        ];
      case 'agent_start':
        return 'demo-session';
      case 'agent_send': {
        const runId = 'demo-live';
        const words = ANSWER.split(/(?<=\s)/);
        let i = 0;
        const step = () => {
          if (i === 0) send('agent_event', { sessionId: 'demo-session', event: { type: 'step_finished', runId, stepId: 's', ok: true, summary: '', detail: { passages: PASSAGES }, durationMs: 1 } });
          if (i < words.length) {
            send('agent_event', { sessionId: 'demo-session', event: { type: 'text_delta', runId, messageId: 'm1', delta: words[i] } });
            i += 1;
            setTimeout(step, 45);
          } else {
            send('agent_event', { sessionId: 'demo-session', event: { type: 'run_finished', runId, status: 'completed', error: null } });
          }
        };
        setTimeout(step, 900);
        return runId;
      }
      case 'agent_approve':
      case 'agent_abort':
        return null;
      default:
        throw { code: 'refused', message: `The sample computer has no ${command}.` };
    }
  };

  return {
    transport: {
      call,
      subscribe: async () => undefined,
      onEvent: async handler => {
        handlers.add(handler);
        return () => handlers.delete(handler);
      },
    },
    status: async () => status,
    onStatus: async () => () => undefined,
    reconnect: async () => undefined,
    pair: async () => undefined,
    reset: async () => undefined,
    // The ready line goes through the real app (main.tsx wraps this).
    ready: async () => undefined,
    startScreen: async () => null,
  };
}

/** Where a demo screenshot opens. */
export function demoNav(screen: string): Nav | null {
  const answer: Nav = { ...initialNav('ask'), ask: [{ kind: 'home' }, { kind: 'answer', conversationId: 'demo-selfrag', question: null, workspaceId: null }] };
  switch (screen) {
    case 'home':
      return initialNav('ask');
    case 'answer':
      return answer;
    case 'conversations':
      return { ...initialNav('ask'), ask: [{ kind: 'home' }, { kind: 'conversations' }] };
    case 'conversations-research':
      return { ...initialNav('ask'), ask: [{ kind: 'home' }, { kind: 'conversations', filter: 'research' }] };
    case 'conversations-code':
      return { ...initialNav('ask'), ask: [{ kind: 'home' }, { kind: 'conversations', filter: 'code' }] };
    case 'conversations-creator':
      return { ...initialNav('ask'), ask: [{ kind: 'home' }, { kind: 'conversations', filter: 'creator' }] };
    case 'code':
      return {
        ...initialNav('ask'),
        ask: [{ kind: 'home' }, { kind: 'conversations', filter: 'code' }, { kind: 'answer', conversationId: 'demo-rerank', question: null, workspaceId: 'w1' }],
      };
    case 'library':
      return { ...initialNav('library'), library: [{ path: ROOT, name: 'Papers' }] };
    case 'reader':
      return {
        ...answer,
        reader: { path: SELF_RAG, name: 'self-rag.pdf', page: 3, passage: PASSAGES[0].text, back: 'Answer', origin: null },
      };
    case 'approvals':
      return initialNav('approvals');
    case 'sheet':
      return { ...initialNav('approvals'), sheet: { sessionId: 'demo-s1', stepId: 'note-1' } };
    case 'workspaces':
      return initialNav('workspaces');
    case 'menu':
      return { ...initialNav('ask'), drawer: true };
    default:
      return null;
  }
}
