/**
 * A Code answer's transcript as the desktop saves it (reducer.ts `toPersisted`):
 * a read, two edits of one file, a write, a refused edit, a command that passed,
 * one that failed with its output cut to the tail, and a step placed among the
 * answer's blocks out of start order.
 */
export const CODE_TRANSCRIPT = {
  runId: 'run-code',
  startedAtMs: 0,
  status: 'completed',
  agentMode: 'ask',
  blocks: [
    { kind: 'step', stepId: 'read' },
    { kind: 'step', stepId: 'edit-a' },
    { kind: 'step', stepId: 'cmd-ok' },
    { kind: 'step', stepId: 'edit-a2' },
    { kind: 'text', id: 'm1', text: 'Empty passages are skipped now, and the test passes.' },
  ],
  steps: {
    read: { id: 'read', tool: 'read', label: 'Read rerank.rs', status: 'done', args: { path: 'src/rerank.rs' }, startedAtMs: 1 },
    'edit-a': { id: 'edit-a', tool: 'edit', label: 'Edit rerank.rs', status: 'done', args: { path: 'src/rerank.rs' }, startedAtMs: 2 },
    'edit-a2': { id: 'edit-a2', tool: 'edit', label: 'Edit rerank.rs', status: 'done', args: { path: 'src/rerank.rs' }, startedAtMs: 5 },
    'write-t': { id: 'write-t', tool: 'write', label: 'Write rerank_empty.rs', status: 'done', args: { path: 'tests/rerank_empty.rs', content: 'fn main() {}' }, startedAtMs: 6 },
    'edit-denied': { id: 'edit-denied', tool: 'edit', label: 'Edit Cargo.toml', status: 'failed', args: { path: 'Cargo.toml' }, startedAtMs: 7 },
    'cmd-ok': {
      id: 'cmd-ok',
      tool: 'run_command',
      label: 'Run cargo test',
      status: 'done',
      args: { command: 'cargo test --test rerank_empty' },
      startedAtMs: 3,
      command: {
        commandId: 'c1',
        command: 'cargo test --test rerank_empty',
        state: 'done',
        startedAtMs: 3,
        timeLimitSecs: 600,
        sandbox: null,
        exitCode: 0,
        note: null,
        durationMs: 1200,
        output: 'test result: ok. 3 passed; 0 failed',
        outputCut: false,
      },
    },
    'cmd-bad': {
      id: 'cmd-bad',
      tool: 'run_command',
      label: 'Run cargo clippy',
      status: 'done',
      args: { command: 'cargo clippy -- -D warnings' },
      startedAtMs: 8,
      command: {
        commandId: 'c2',
        command: 'cargo clippy -- -D warnings',
        state: 'done',
        startedAtMs: 8,
        timeLimitSecs: 600,
        sandbox: null,
        exitCode: 101,
        note: null,
        durationMs: 900,
        output: 'error: unused variable `x`',
        outputCut: true,
      },
    },
  },
  passages: [],
  groundings: [],
  interimIds: [],
};

export const CODE_CONVERSATION = {
  id: 'code-1',
  title: 'Make the reranker skip empty passages',
  updatedAt: '2026-11-14T08:00:00.000Z',
  workspaceId: 'w1',
  mode: 'code',
  agentMode: 'ask',
  messages: [
    { id: 'q', role: 'user', content: 'Make the reranker skip empty passages', timestamp: '2026-11-14T08:00:00.000Z' },
    { id: 'a', role: 'assistant', content: 'Empty passages are skipped now, and the test passes.', timestamp: '2026-11-14T08:01:00.000Z', transcript: CODE_TRANSCRIPT },
  ],
};
