import { formatCost, formatMs, formatTokens } from '../agent/format';
import type { AuditRow } from './types';

function str(value: unknown): string | null {
  return typeof value === 'string' && value.trim() !== '' ? value : null;
}

function num(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

function clip(text: string, max = 120): string {
  const single = text.replace(/\s+/g, ' ').trim();
  return single.length > max ? `${single.slice(0, max - 1)}…` : single;
}

/** Last path component, for compact display. */
function baseName(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.length > 0 ? parts[parts.length - 1] : path;
}

const SOURCE_ACTIONS: Record<string, string> = {
  index_folder: 'Indexed folder',
  add_file: 'Added file',
  add: 'Added folder',
  reindex: 'Re-indexed',
  remove: 'Removed',
  clear_all: 'Cleared all documents',
};

/** One plain-language line describing an event. */
export function summarise(row: AuditRow): string {
  const p = row.payload ?? {};
  switch (row.eventType) {
    case 'question': {
      const text = str(p.text) ?? '';
      const prefix = p.steer === true ? 'Redirected: ' : '';
      return `${prefix}“${clip(text)}”`;
    }
    case 'answer': {
      const parts: string[] = [str(p.status) ?? 'finished'];
      const tin = num(p.tokens_in);
      const tout = num(p.tokens_out);
      if (tin !== null || tout !== null) parts.push(`${formatTokens(tin ?? 0)} in / ${formatTokens(tout ?? 0)} out`);
      const cost = formatCost(num(p.cost_usd) ?? 0);
      if (cost) parts.push(cost);
      const citations = Array.isArray(p.citations) ? p.citations.length : 0;
      parts.push(`${citations} ${citations === 1 ? 'citation' : 'citations'}`);
      const duration = num(p.duration_ms);
      if (duration !== null) parts.push(formatMs(duration));
      return parts.join(' · ');
    }
    case 'tool_call': {
      const parts: string[] = [str(p.tool) ?? 'tool'];
      const summary = str(p.summary);
      if (summary) parts.push(clip(summary, 80));
      if (p.ok === false) parts.push('failed');
      const duration = num(p.duration_ms);
      if (duration !== null) parts.push(formatMs(duration));
      return parts.join(' · ');
    }
    case 'retrieval': {
      if (p.tool === 'open_document') {
        const path = str(p.path);
        const location = str(p.location);
        return `Opened ${path ? baseName(path) : 'a document'}${location ? ` · ${location}` : ''}`;
      }
      const passages = Array.isArray(p.passages) ? p.passages.length : 0;
      const query = str(p.query);
      return `${passages} ${passages === 1 ? 'passage' : 'passages'}${query ? ` for “${clip(query, 80)}”` : ''}`;
    }
    case 'approval': {
      const decision = str(p.decision) ?? 'decided';
      const label = str(p.label) ?? str(p.tool) ?? 'step';
      const decided: Record<string, string> = {
        approved: 'Approved',
        denied: 'Declined',
        timed_out: 'Timed out',
        cancelled: 'Cancelled',
        refused: 'Refused',
      };
      return `${decided[decision] ?? decision}: ${clip(label, 100)}`;
    }
    case 'source_change': {
      const action = str(p.action) ?? 'changed';
      const target = str(p.path) ?? str(p.source_id);
      const via = p.via === 'agent' ? ' (agent)' : '';
      const failed = p.ok === false ? ' · failed' : '';
      const files = num(p.files);
      const count = files !== null ? ` · ${files} ${files === 1 ? 'file' : 'files'}` : '';
      return `${SOURCE_ACTIONS[action] ?? action}${target ? ` ${clip(target, 90)}` : ''}${via}${count}${failed}`;
    }
    case 'settings_change': {
      const action = str(p.action);
      if (action === 'model_switch') {
        const provider = str(p.provider);
        const model = str(p.model);
        const target = [provider, model].filter(Boolean).join(' / ') || str(p.mode) || 'disabled';
        return `Model → ${target}${p.ok === false ? ' · failed' : ''}`;
      }
      if (action === 'api_key_set') return `API key saved for ${str(p.provider) ?? 'a provider'}`;
      if (action === 'api_key_deleted') return `API key removed for ${str(p.provider) ?? 'a provider'}`;
      if (p.setting === 'audit_retention_days') {
        return `Audit retention ${num(p.old) ?? '?'} → ${num(p.new) ?? '?'} days`;
      }
      return action ?? 'Setting changed';
    }
    case 'runtime_install': {
      const version = str(p.version) ?? '';
      if (p.ok === false) return `Agent runtime ${version} install failed · ${clip(str(p.error) ?? '', 80)}`;
      const sha = str(p.sha256);
      return `Agent runtime ${version} installed${sha ? ` · sha256 ${sha.slice(0, 12)}…` : ''}`;
    }
    case 'memory_write': {
      const actions: Record<string, string> = {
        remember: 'Remembered',
        update: 'Memory edited',
        pin: 'Memory pinned',
        unpin: 'Memory unpinned',
      };
      const action = str(p.action) ?? 'remember';
      const outcome = str(p.outcome);
      const label = outcome === 'unchanged' ? 'Already remembered' : actions[action] ?? 'Memory changed';
      const via = p.via === 'agent' ? ' (agent, approved)' : '';
      const text = str(p.text);
      return `${label}${via}${text ? `: “${clip(text, 90)}”` : ''}`;
    }
    case 'memory_forget': {
      const versions = Array.isArray(p.ids) ? p.ids.length : 1;
      const text = str(p.text);
      return `Forgot ${versions} ${versions === 1 ? 'version' : 'versions'}${text ? ` of “${clip(text, 90)}”` : ''}`;
    }
    case 'memory_use': {
      const count = Array.isArray(p.ids) ? p.ids.length : 0;
      const query = str(p.query);
      return `${count} ${count === 1 ? 'memory' : 'memories'} recalled${query ? ` for “${clip(query, 80)}”` : ''}`;
    }
    case 'code_change': {
      const branch = str(p.branch) ?? 'a branch';
      const base = str(p.base) ?? 'the original branch';
      return p.action === 'discarded'
        ? `Discarded Code changes: back on ${base} (kept on ${branch})`
        : `Code changes go to ${branch} (from ${base})`;
    }
    case 'retention_checkpoint': {
      const deleted = num(p.deleted) ?? 0;
      return `Retention removed ${deleted} ${deleted === 1 ? 'event' : 'events'} (through #${num(p.last_deleted_id) ?? '?'})`;
    }
    default:
      return clip(JSON.stringify(p));
  }
}

/** "Oct 3, 14:05:09" in local time. */
export function formatTimestamp(ts: string): string {
  const date = new Date(ts);
  if (Number.isNaN(date.getTime())) return ts;
  return date.toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  });
}

/** Local `YYYY-MM-DD` (for date inputs and file names). */
export function localDate(date: Date): string {
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, '0');
  const d = String(date.getDate()).padStart(2, '0');
  return `${y}-${m}-${d}`;
}

/** Start (00:00) or end (23:59:59.999) of a local `YYYY-MM-DD` as ISO. */
export function dayBoundary(value: string, edge: 'start' | 'end'): string | undefined {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  if (!match) return undefined;
  const [, y, m, d] = match;
  const date =
    edge === 'start'
      ? new Date(Number(y), Number(m) - 1, Number(d), 0, 0, 0, 0)
      : new Date(Number(y), Number(m) - 1, Number(d), 23, 59, 59, 999);
  return Number.isNaN(date.getTime()) ? undefined : date.toISOString();
}
