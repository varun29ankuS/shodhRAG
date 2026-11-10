/**
 * Recall's words and choices for the Library card and the session viewer. Pure (type
 * imports only) so Node's strip-types runner tests it (`app/tests/recall.test.ts`).
 */

import type { RecallAssistantAccess, RecallAssistantModel, RecallRole, RecallSettings, RecallStatus, RecallToolId } from './api';

/** Where the card is in its setup. */
export type RecallStage =
  /** Off: nothing is read. */
  | 'off'
  /** On, but no tool chosen. */
  | 'tools'
  /** Tools chosen, no project included yet (projects are listed from the transcripts' first lines). */
  | 'projects'
  /** Reading the chosen projects. */
  | 'ready';

export function recallStage(status: RecallStatus): RecallStage {
  const s = status.settings;
  if (!s.enabled) return 'off';
  if (s.tools.length === 0) return 'tools';
  if (!Object.values(s.projects).some(Boolean)) return 'projects';
  return 'ready';
}

export function toolLabel(id: RecallToolId): string {
  return id === 'claude_code' ? 'Claude Code' : 'Codex';
}

/** The settings with `tool` chosen or not. */
export function withTool(settings: RecallSettings, tool: RecallToolId, chosen: boolean): RecallSettings {
  const tools = settings.tools.filter(t => t !== tool);
  if (chosen) tools.push(tool);
  tools.sort();
  return { ...settings, tools };
}

/** The settings with a project included or excluded. */
export function withProject(settings: RecallSettings, key: string, included: boolean): RecallSettings {
  return { ...settings, projects: { ...settings.projects, [key]: included } };
}

/** The model the assistant answers with, in plain words: "claude-sonnet-4 (anthropic)". */
export function modelWords(model: RecallAssistantModel | null): string {
  if (!model) return 'the model you choose';
  return model.model ? `${model.model} (${model.provider})` : model.provider;
}

/** What the card says about what the assistant may read, and where it goes. */
export function assistantLine(access: RecallAssistantAccess, model: RecallAssistantModel | null): string {
  if (access === 'off') return 'The assistant cannot read your coding sessions.';
  const where = `When you ask the assistant about your sessions, the cleaned text it opens goes to ${modelWords(model)}${model?.cloud ? ', off this computer' : ''}.`;
  if (access === 'local_models') {
    return model && !model.cloud
      ? `${where} It reads them only while you use a model on this computer.`
      : 'The assistant reads your coding sessions only while you use a model on this computer, and you do not now.';
  }
  return where;
}

/** Sessions being indexed again under new hidden words, unsearchable meanwhile. */
export function staleLine(status: RecallStatus): string | null {
  if (status.staleSessions === 0) return null;
  return `${plural(status.staleSessions, 'session')} ${status.staleSessions === 1 ? 'is' : 'are'} being indexed again under your hidden words, and cannot be searched until then.`;
}

/** One pattern a line, blank lines dropped. */
export function parsePatterns(text: string): string[] {
  return text
    .split(/\r?\n/)
    .map(line => line.trim())
    .filter(line => line.length > 0);
}

function plural(n: number, one: string, many = `${one}s`): string {
  return `${n.toLocaleString()} ${n === 1 ? one : many}`;
}

/** "3 minutes ago" for an RFC 3339 time, relative to `now`. */
export function agoText(at: string | null, now: Date): string | null {
  if (!at) return null;
  const then = Date.parse(at);
  if (Number.isNaN(then)) return null;
  const minutes = Math.max(0, Math.round((now.getTime() - then) / 60_000));
  if (minutes < 1) return 'just now';
  if (minutes < 60) return `${plural(minutes, 'minute')} ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${plural(hours, 'hour')} ago`;
  return `${plural(Math.round(hours / 24), 'day')} ago`;
}

/** The card's one-line state, in plain words. */
export function recallStatusLine(status: RecallStatus, now: Date): string {
  const run = status.running;
  if (run) {
    const where = run.current ? ` (${run.current})` : '';
    if (run.paused) return `Paused: ${run.done.toLocaleString()} of ${plural(run.total, 'session')} indexed${where}`;
    if (run.total === 0) return 'Looking for new sessions';
    return `Indexing ${run.done.toLocaleString()} of ${plural(run.total, 'session')}${where}`;
  }
  const parts = [`${plural(status.indexedSessions, 'session')} indexed`];
  if (status.secretsRedacted > 0) parts.push(`${plural(status.secretsRedacted, 'secret')} redacted`);
  const checked = agoText(status.lastSync?.at ?? null, now);
  if (checked) parts.push(`checked ${checked}`);
  return parts.join(' · ');
}

/** Sessions changed too recently to index (their tool may still write to them). */
export function waitingLine(status: RecallStatus): string | null {
  const waiting = status.lastSync?.waiting ?? 0;
  if (waiting === 0) return null;
  return `${plural(waiting, 'session')} still in use: indexed once idle for ${plural(status.settings.idleMinutes, 'minute')}`;
}

/** A turn's role, as the viewer labels it. */
export function roleLabel(role: RecallRole, toolName: string | null): string {
  switch (role) {
    case 'user':
      return 'You';
    case 'assistant':
      return 'Assistant';
    case 'tool_call':
      return toolName ? `Tool call: ${toolName}` : 'Tool call';
    case 'tool_output':
      return toolName ? `Output of ${toolName}` : 'Tool output';
  }
}

/** `2026-10-12` of an RFC 3339 time, or null. */
export function dayOf(at: string | null): string | null {
  return at && /^\d{4}-\d{2}-\d{2}/.test(at) ? at.slice(0, 10) : null;
}

/** "2026-10-12, 14:03" in UTC, for a turn's time. */
export function timeOf(at: string | null): string | null {
  if (!at || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}/.test(at)) return null;
  return `${at.slice(0, 10)}, ${at.slice(11, 16)}`;
}

/** A receipt id is `<file>:<turn>`. */
export function isReceipt(value: string | null | undefined): value is string {
  return typeof value === 'string' && /^\d{1,12}:\d{1,7}$/.test(value);
}
