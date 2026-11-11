/**
 * Activity entries in plain words (src/features/audit/sentence.ts): every event type
 * and every action the app records reads as a sentence, never as an id or raw JSON.
 * The actions are read from the Rust sources, so a new one without a sentence fails here.
 *   node --experimental-strip-types --test app/tests/auditSentence.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describeEvent, humanize, sentenceText } from '../src/features/audit/sentence.ts';
import type { DescribedEvent } from '../src/features/audit/sentence.ts';
import { EVENT_TYPES, eventLabel } from '../src/features/audit/types.ts';

const tauriSrc = fileURLToPath(new URL('../src-tauri/src/', import.meta.url));
const crateSrc = fileURLToPath(new URL('../../crates/shodh-rag/src/', import.meta.url));

function rustFiles(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) out.push(...rustFiles(path));
    else if (name.endsWith('.rs')) out.push(path);
  }
  return out;
}

/** One entry of every kind the app writes, with the payload its Rust emitter builds. */
const ENTRIES: readonly DescribedEvent[] = [
  { eventType: 'question', payload: { text: 'What is the notice period?' } },
  { eventType: 'question', payload: { text: 'Shorter, please', steer: true } },
  { eventType: 'answer', payload: { status: 'completed', tokens_in: 1200, tokens_out: 300, cost_usd: 0.012, citations: [1, 2], duration_ms: 4200 } },
  { eventType: 'answer', payload: { status: 'aborted', citations: [] } },
  { eventType: 'answer', payload: { status: 'error', error: 'omp exited', citations: [] } },
  { eventType: 'tool_call', payload: { tool: 'search_documents', ok: true, summary: '8 passages', duration_ms: 320 } },
  { eventType: 'tool_call', payload: { tool: 'mcp__github__create_issue', ok: false, summary: 'Not allowed', duration_ms: 90 } },
  { eventType: 'retrieval', payload: { tool: 'search_documents', query: 'notice period', passages: [{}, {}] } },
  { eventType: 'retrieval', payload: { tool: 'open_document', path: 'C:/Docs/contract.pdf', location: 'page 4', passages: [] } },
  { eventType: 'retrieval', payload: { tool: 'web_search', query: 'tauri updater', sources: [{}, {}, {}] } },
  { eventType: 'retrieval', payload: { tool: 'search_papers', query: 'vector quantisation', sources: [{}] } },
  { eventType: 'retrieval', payload: { tool: 'fetch_url', url: 'https://example.org/page', final_url: 'https://www.example.org/page', sources: [] } },
  ...['approved', 'denied', 'timed_out', 'cancelled', 'refused'].map(decision => ({
    eventType: 'approval',
    payload: { tool: 'write_file', label: 'Write notes.md', decision, who: 'local' },
  })),
  { eventType: 'approval', payload: { tool: 'run_command', label: 'Run cargo test', decision: 'auto_approved', mode: 'auto' } },
  ...['index_folder', 'add', 'add_file', 'reindex', 'remove', 'clear_all', 'reset', 'create_folder', 'download'].map(action => ({
    eventType: 'source_change',
    payload: { action, via: 'ui', source_id: 'space-1', path: 'C:/Docs/Papers', ok: true, files: 12 },
  })),
  { eventType: 'source_change', payload: { action: 'index_folder', via: 'ui', path: 'C:/Docs', ok: true, files: 0, files_unchanged: 40 } },
  { eventType: 'source_change', payload: { action: 'indexing_cancelled', sourceId: 'space-1', runs: 2, via: 'agent' } },
  // Settings
  { eventType: 'settings_change', payload: { action: 'preference_change', key: 'theme', old: 'dark', new: 'light', via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'preference_change', key: 'search_max_results', old: 8, new: 12, via: 'agent' } },
  { eventType: 'settings_change', payload: { action: 'policy_change', old: { localOnly: false, webAccess: true }, new: { localOnly: true, webAccess: true }, via: 'ui' } },
  {
    eventType: 'settings_change',
    payload: {
      action: 'memory_preferences_change',
      old: { injectMemories: true, learnMode: 'auto', learnModel: null, autoMinConfidence: 0.8, learnCaps: { maxCallsPerDay: 10, maxInputCharsPerDay: 60000, maxProposalsPerDay: 20 } },
      new: { injectMemories: true, learnMode: 'ask', learnModel: 'gpt-4o-mini', autoMinConfidence: 0.9, learnCaps: { maxCallsPerDay: 20, maxInputCharsPerDay: 60000, maxProposalsPerDay: 20 } },
      via: 'ui',
    },
  },
  { eventType: 'settings_change', payload: { action: 'answer_preferences_change', old: { autoRepair: true, thinkBeforeAnswering: false }, new: { autoRepair: true, thinkBeforeAnswering: true }, via: 'ui' } },
  {
    eventType: 'settings_change',
    payload: {
      action: 'notification_preferences_change',
      old: { reminders: true, needsYou: true, memorySuggestions: true, workFinished: true, problems: true },
      new: { reminders: true, needsYou: false, memorySuggestions: true, workFinished: true, problems: true },
      via: 'ui',
    },
  },
  { eventType: 'settings_change', payload: { action: 'tables_preferences_change', old: { dailyCallCap: 30, diagnostics: false }, new: { dailyCallCap: 60, diagnostics: false }, via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'code_mode_change', old: false, new: true, via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'default_mode_change', old: 'ask', new: 'acceptEdits', via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'close_to_tray', old: false, new: true, via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'agent_idle_minutes', old: 10, new: 0, via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'start_with_windows', old: false, new: true, via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'check_for_updates', old: true, new: false, via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'app_update_install', old: '0.0.10', new: '0.0.11', via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'memory_learning_stop', old: 'auto', new: 'off', discarded: 3, via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'code_settings_change', workspace: 'ws-1', new: { allowlist: ['cargo test', 'npm test'] }, via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'code_build_trust', workspace: 'ws-1', trusted: true, via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'code_build_trust', workspace: 'ws-1', trusted: false, via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'provider_connection', provider: 'claude-sub', change: 'sign_in' } },
  { eventType: 'settings_change', payload: { action: 'provider_connection', provider: 'chatgpt-sub', change: 'sign_out' } },
  { eventType: 'settings_change', payload: { action: 'api_key_set', provider: 'openrouter' } },
  { eventType: 'settings_change', payload: { action: 'api_key_deleted', provider: 'anthropic' } },
  { eventType: 'settings_change', payload: { action: 'model_switch', mode: 'external', provider: 'openrouter', model: 'meta-llama/llama-3.3-70b-instruct', cloud: true } },
  { eventType: 'settings_change', payload: { action: 'model_switch', mode: 'local', provider: 'local', model: 'qwen2.5-7b.gguf', cloud: false } },
  { eventType: 'settings_change', payload: { action: 'model_switch', mode: 'disabled', provider: null, model: null, cloud: false } },
  { eventType: 'settings_change', payload: { action: 'indexing_paused', sourceId: null, runs: null, via: 'agent' } },
  { eventType: 'settings_change', payload: { action: 'indexing_resumed', sourceId: null, runs: null, via: 'agent' } },
  { eventType: 'settings_change', payload: { setting: 'audit_retention_days', old: 90, new: 30 } },
  // Connectors (settings_change with the connector's action)
  { eventType: 'settings_change', payload: { action: 'github_sign_in', login: 'octocat', via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'github_sign_out', login: 'octocat', via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'github_add_repo', repo: 'octo/repo', write: false, merge: false, via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'github_set_repo', repo: 'octo/repo', write: true, merge: true, before: { write: true, merge: false }, via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'github_remove_repo', repo: 'octo/repo', via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'github_grant', repo: 'octo/repo', write: true, merge: false, via: 'approval' } },
  { eventType: 'settings_change', payload: { action: 'slack_connect', slackWorkspace: 'Acme', bot: 'shodh', via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'slack_check_permissions', slackWorkspace: 'Acme', scopes: ['chat:write', 'channels:history'], via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'slack_set_enabled', enabled: true, via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'slack_set_answer_workspace', workspaceId: 'ws-1', workspace: 'Research', via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'slack_set_answer_channels', channels: ['general', 'papers'], sharedWithOtherOrganizations: true, via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'slack_set_daily_cap', old: 50, new: 100, via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'slack_disconnect', slackWorkspace: 'Acme', forgot: true, threadsForgotten: 4, memoriesForgotten: 2, via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'slack_index_channel', channel: 'papers', workspace: 'Research', sharedWithOtherOrganizations: false, windowDays: 30, via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'slack_forget_channel', channel: 'papers', threads: 12, memories: 1, conversations: 3, via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'slack_set_index_window', days: 90, via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'slack_set_owner', owner: 'Varun', via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'slack_promote_memory', channel: 'papers', author: 'Asha', via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'slack_delete_memory', channel: 'papers', author: 'Asha', via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'telegram_connect', bot: 'ShodhBot', via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'telegram_link_code', kind: 'owner', expiresAt: '2026-11-11T10:00:00Z', via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'telegram_link_code', kind: 'invite', expiresAt: '2026-11-11T10:00:00Z', via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'telegram_owner_linked', name: 'Varun', via: 'telegram' } },
  { eventType: 'settings_change', payload: { action: 'telegram_person_joined', name: 'Asha', via: 'telegram' } },
  { eventType: 'settings_change', payload: { action: 'telegram_unlink_owner', via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'telegram_set_enabled', enabled: false, via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'telegram_set_answer_workspace', workspaceId: 'ws-1', workspace: 'Research', via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'telegram_allow_group', group: 'Lab', via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'telegram_remove_group', group: 'Lab', via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'telegram_remove_person', name: 'Asha', via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'telegram_set_daily_cap', old: 20, new: 40, via: 'ui' } },
  { eventType: 'settings_change', payload: { action: 'telegram_disconnect', bot: 'ShodhBot', forgot: false, conversationsForgotten: null, via: 'ui' } },
  // Runtime and retention
  { eventType: 'runtime_install', payload: { version: '13.1.0', sha256: 'abc', ok: true } },
  { eventType: 'runtime_install', payload: { version: '13.1.0', ok: false, error: 'checksum mismatch' } },
  { eventType: 'runtime_install', payload: { component: 'search_models', ok: true } },
  { eventType: 'runtime_install', payload: { component: 'search_models', ok: false, error: 'disk full' } },
  { eventType: 'retention_checkpoint', payload: { deleted: 120, last_deleted_id: 4410 } },
  // Memories
  ...['remember', 'update', 'pin', 'unpin', 'not_relevant', 'archive', 'unarchive', 'resolve_conflict', 'undo_resolve_conflict', 'undo_learned'].map(action => ({
    eventType: 'memory_write',
    payload: { action, id: 'm1', class: 'preference', scope: 'global', text_chars: 42, via: 'ui' },
  })),
  { eventType: 'memory_write', payload: { action: 'remember', outcome: 'unchanged', class: 'project', via: 'agent' } },
  { eventType: 'memory_write', payload: { action: 'remember', outcome: 'added', text: 'Prefers metric units', via: 'agent' } },
  { eventType: 'memory_write', payload: { action: 'link', ids: ['m1', 'm2'], reason_chars: 30, extractor: 'llm', via: 'learn' } },
  { eventType: 'memory_write', payload: { action: 'undo_link', ids: ['m1', 'm2'], extractor: 'llm', via: 'ui' } },
  { eventType: 'memory_forget', payload: { action: 'forget', id: 'm1', versions: 2, ids: ['m1', 'm0'], class: 'preference', via: 'ui' } },
  { eventType: 'memory_use', payload: { action: 'recall', ids: ['m1', 'm2', 'm3'], query: 'units' } },
  { eventType: 'memory_use', payload: { action: 'used_in_answer', ids: ['m1'], considered: 3 } },
  // Models, code, citations, scopes
  {
    eventType: 'model_change',
    payload: {
      from: 'openrouter/meta-llama/llama-3.3-70b-instruct:free',
      to: 'anthropic/claude-sonnet',
      reason: 'fallback_once',
      from_model: { provider: 'openrouter', model: 'llama-3.3-70b-instruct:free' },
      to_model: { provider: 'anthropic', model: 'claude-sonnet' },
      cloud: true,
      failure: 'rate_limited',
    },
  },
  ...['user_choice', 'session_override', 'fallback_automatic', 'startup'].map(reason => ({
    eventType: 'model_change',
    payload: { from: 'none', to: 'lmstudio/qwen', reason, from_model: null, to_model: { provider: 'lmstudio', model: 'qwen2.5-7b' }, cloud: false },
  })),
  { eventType: 'code_change', payload: { action: 'created', branch: 'shodh/notes', base: 'main', folder: 'C:/code/app' } },
  { eventType: 'code_change', payload: { action: 'discarded', branch: 'shodh/notes', base: 'main', folder: 'C:/code/app' } },
  { eventType: 'citation_corrected', payload: { from: [2], to: 3, score: 0.82 } },
  { eventType: 'scope_change', payload: { moved: { memories: 2, snippets: 0, results: 1, others: 0 }, reason: 'workspace deleted' } },
];

/** What no sentence may show: ids, JSON, or values that were never filled in. */
function unreadable(text: string): string | null {
  const id = /\b[a-z]+_[a-z_]+\b/.exec(text);
  if (id) return `an id (${id[0]})`;
  if (/[{}"]/.test(text)) return 'JSON';
  const blank = /\b(undefined|null|NaN)\b|\[object/.exec(text);
  if (blank) return `a missing value (${blank[0]})`;
  return null;
}

test('the event types are the ones the Rust audit log records', () => {
  const rust = readFileSync(join(crateSrc, 'audit', 'mod.rs'), 'utf8');
  const asStr = rust.slice(rust.indexOf('pub fn as_str(self)'));
  const ids = [...asStr.slice(0, asStr.indexOf('\n    }\n')).matchAll(/AuditEventType::\w+ => "([a-z_]+)"/g)].map(m => m[1]);
  assert.ok(ids.length >= 16, `read ${ids.length} types from audit/mod.rs`);
  assert.deepEqual([...EVENT_TYPES.map(t => t.id)].sort(), [...ids].sort());
  for (const id of ids) {
    assert.notEqual(eventLabel(id), id, `${id} has a label`);
    assert.ok(ENTRIES.some(e => e.eventType === id), `${id} has an example entry`);
  }
});

test('every entry reads as a sentence', () => {
  for (const entry of ENTRIES) {
    const sentence = describeEvent(entry);
    const text = sentenceText(sentence);
    const where = `${entry.eventType} ${JSON.stringify(entry.payload)} → ${text}`;
    assert.equal(sentence.known, true, `no sentence: ${where}`);
    assert.ok(sentence.title.length > 0, where);
    const problem = unreadable(text);
    assert.equal(problem, null, `${problem}: ${where}`);
  }
});

test('every action the Rust code records has an example entry with a sentence', () => {
  const patterns = [
    /"action":\s*"([a-z_]+)"/g,
    // Connector and memory helpers: `audit_change(&audit, "slack_connect", …)`.
    /audit_change\(\s*&?[\w.:<>()]+,\s*"([a-z_]+)"/g,
    /control_payload\(\s*"([a-z_]+)"/g,
    /source_change(?:_record)?\(\s*"([a-z_]+)"/g,
  ];
  // Telegram's "typing…" indicator is a Bot API call, not an audit entry.
  const notAudit = new Set(['typing']);
  const actions = new Set<string>();
  for (const file of [...rustFiles(tauriSrc), ...rustFiles(crateSrc)]) {
    const source = readFileSync(file, 'utf8');
    for (const pattern of patterns) for (const m of source.matchAll(pattern)) actions.add(m[1]);
  }
  for (const known of notAudit) actions.delete(known);
  assert.ok(actions.size > 60, `read ${actions.size} actions from the Rust sources`);
  const covered = new Set(ENTRIES.map(e => e.payload?.action).filter((a): a is string => typeof a === 'string'));
  const missing = [...actions].filter(a => !covered.has(a)).sort();
  assert.deepEqual(missing, [], 'actions with no example entry (add one, and a sentence in sentence.ts)');
});

test('a settings change says what changed, the new value emphasised', () => {
  const sentence = describeEvent({ eventType: 'settings_change', payload: { action: 'preference_change', key: 'theme', old: 'dark', new: 'light', via: 'ui' } });
  assert.equal(sentenceText(sentence), 'Settings changed · Theme: Dark → Light');
  assert.equal(sentence.icon, 'settings');
  assert.deepEqual(sentence.details.filter(d => d.strong).map(d => d.text), ['Light']);
});

test('a preference group lists only the fields that changed', () => {
  const sentence = describeEvent({
    eventType: 'settings_change',
    payload: { action: 'policy_change', old: { localOnly: false, webAccess: true }, new: { localOnly: true, webAccess: true }, via: 'ui' },
  });
  assert.equal(sentenceText(sentence), 'Privacy changed · Local-only mode: Off → On');
});

test('an action this version does not know still reads as words', () => {
  const sentence = describeEvent({ eventType: 'settings_change', payload: { action: 'future_thing_change' } });
  assert.equal(sentence.known, false);
  assert.equal(sentenceText(sentence), 'Settings changed · Future thing change');
  const type = describeEvent({ eventType: 'brand_new_type', payload: { a: 1 } });
  assert.equal(type.known, false);
  assert.equal(unreadable(sentenceText(type)), null);
});

test('ids read as words', () => {
  assert.equal(humanize('search_documents'), 'Search documents');
  assert.equal(humanize('learnModel'), 'Learn model');
  assert.equal(humanize(''), '');
});
