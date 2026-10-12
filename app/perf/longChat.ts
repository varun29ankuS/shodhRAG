/**
 * Measurement harness for long conversations (not part of the app build).
 * Runs the real app against a mocked backend whose conversations are a
 * synthetic 300+ message conversation and a short one; `window.__perf`
 * lets the CDP driver (perf/measure.mjs) stream a synthetic answer.
 */
import { mockIPC } from '@tauri-apps/api/mocks';
import { emit } from '@tauri-apps/api/event';
import { longConversation, shortConversation } from './fixture';

const TURNS = Number(new URLSearchParams(location.search).get('turns') ?? '160');

interface PerfApi {
  commands: Record<string, number>;
  sends: number;
  /** Stream `deltas` text chunks into the running answer, one every `intervalMs`. */
  stream: (deltas: number, intervalMs: number) => Promise<void>;
}

const conversations = [shortConversation(), longConversation(TURNS)];
let lastSend: { sessionId: string; runId: string } | null = null;

const perf: PerfApi = {
  commands: {},
  sends: 0,
  async stream(deltas, intervalMs) {
    const send = lastSend;
    if (!send) throw new Error('No answer is running');
    const event = (e: Record<string, unknown>) => emit('agent_event', { sessionId: send.sessionId, event: { runId: send.runId, ...e } });
    await event({ type: 'run_started', sessionId: send.sessionId, model: 'synthetic/model', atMs: Date.now(), warning: null });
    const words = 'Streaming answers keep the newest text in view while the reader stays at the bottom of the conversation [1]. '.split(' ');
    for (let i = 0; i < deltas; i++) {
      const delta = (i % 40 === 39 ? '\n\n' : '') + words[i % words.length] + ' ';
      await event({ type: 'text_delta', messageId: 'm1', delta });
      await new Promise(r => setTimeout(r, intervalMs));
    }
    await event({ type: 'run_finished', status: 'completed', durationMs: deltas * intervalMs, error: null });
  },
};
(window as unknown as { __perf: PerfApi }).__perf = perf;

function respond(cmd: string, args: Record<string, unknown> | undefined): unknown {
  switch (cmd) {
    case 'load_conversations':
      return conversations;
    case 'save_conversation':
    case 'rename_conversation':
    case 'pin_conversation':
    case 'delete_conversation':
      return null;
    case 'agent_runtime_status':
      return { installed: true, path: 'synthetic', version: '0.0.0' };
    case 'agent_session_counts':
      return { main: 0, side: 0, total: 0 };
    case 'agent_start':
      return 'sess-perf';
    case 'agent_send': {
      perf.sends += 1;
      lastSend = { sessionId: String(args?.sessionId), runId: String(args?.requestId) };
      return lastSend.runId;
    }
    case 'get_llm_info':
      return { provider: 'synthetic', model: 'synthetic/model' };
    case 'get_statistics':
      return { total_documents: 9, total_chunks: 900, index_size_mb: '12' };
    case 'search_models_status':
      return { ready: true, installing: false, artifacts: [] };
    case 'workspaces_list':
    case 'inbox_list':
    case 'list_missed_reminders':
      return [];
    case 'tools_for_chat':
      return { servers: [], skills: [], builtin: 12, total: 12, many: false };
    default:
      return null;
  }
}

mockIPC((cmd, args) => {
  perf.commands[cmd] = (perf.commands[cmd] ?? 0) + 1;
  return respond(cmd, args as Record<string, unknown> | undefined);
}, { shouldMockEvents: true });

// Setup is done: the welcome flow would cover the conversation.
localStorage.setItem('shodh.firstRun.status', 'completed');

void import('../src/main.tsx');
