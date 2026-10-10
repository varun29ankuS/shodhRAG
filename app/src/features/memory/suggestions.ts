/**
 * Suggested memories (learning from conversations) as Settings → Memory and the composer
 * badge show them: parsing the backend's `ProposalView` (`shodh_rag::user_memory::learn`),
 * describing each decision ("Replaces: lives in Delhi → Pune"), edits, and the learning
 * settings. Pure functions only, so they run under `node --test` without React or Tauri.
 */
import type { MemoryContent } from './model.ts';

export type LearnMode = 'off' | 'ask' | 'auto';

export type SuggestionStatus = 'pending' | 'accepted' | 'rejected' | 'learned' | 'undone' | 'stale' | 'failed';

export type SuggestionKind = 'remember' | 'revise' | 'link' | 'resolve' | 'archive';

export type SensitiveReason = 'health' | 'financial_identifier' | 'credential' | 'third_party_personal';

export interface ValueChange {
  property: string;
  label: string;
  from: string;
  to: string;
}

/** How a suggested memory relates to what is remembered (mirror of `Decision`). */
export type Decision =
  | { kind: 'add' }
  | { kind: 'noop'; existing: string }
  | { kind: 'extend'; target: string }
  | { kind: 'supersede'; target: string; changes: ValueChange[]; explicit: boolean }
  | { kind: 'update'; target: string }
  | { kind: 'historical'; current: string };

/** Raw fact content as the backend stores it (snake_case tags, `valid_from`). */
export interface RawFact {
  kind: 'fact';
  class: string;
  subject: { id: string; class?: string } | null;
  properties: Record<string, unknown>;
  valid_from?: string | null;
}

export type SuggestionAction =
  | {
      kind: 'remember';
      content: RawFact | { kind: 'note'; text: string };
      text: string;
      decision: Decision;
      decided_by: 'rule' | 'model' | 'undecided';
      target_text: string | null;
      evidence: string;
    }
  | { kind: 'revise'; target: string; target_text: string; content: RawFact; text: string; reason: string }
  | { kind: 'link'; a: string; b: string; a_text: string; b_text: string; reason: string }
  | { kind: 'resolve'; keep: string; keep_text: string; retire: string; retire_text: string; changes: ValueChange[] }
  | { kind: 'archive'; target: string; text: string; strength: number };

export interface Suggestion {
  id: string;
  kind: SuggestionKind;
  origin: 'turn' | 'evolve' | 'consolidate';
  status: SuggestionStatus;
  action: SuggestionAction;
  confidence: number;
  sensitive: SensitiveReason[];
  conversationId: string | null;
  turnId: string | null;
  /** Where the memory goes: `global` or `workspace:<id>`. */
  scope: string;
  error: string | null;
  undoable: boolean;
  createdAt: string;
  decidedAt: string | null;
}

const STATUSES: readonly SuggestionStatus[] = ['pending', 'accepted', 'rejected', 'learned', 'undone', 'stale', 'failed'];
const KINDS: readonly SuggestionKind[] = ['remember', 'revise', 'link', 'resolve', 'archive'];
const ORIGINS = ['turn', 'evolve', 'consolidate'] as const;
const SENSITIVE: readonly SensitiveReason[] = ['health', 'financial_identifier', 'credential', 'third_party_personal'];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function str(value: unknown): value is string {
  return typeof value === 'string';
}

function parseChanges(value: unknown): ValueChange[] | null {
  if (!Array.isArray(value)) return null;
  const out: ValueChange[] = [];
  for (const c of value) {
    if (!isRecord(c) || !str(c.property) || !str(c.label) || !str(c.from) || !str(c.to)) return null;
    out.push({ property: c.property, label: c.label, from: c.from, to: c.to });
  }
  return out;
}

function parseDecision(value: unknown): Decision | null {
  if (!isRecord(value) || !str(value.kind)) return null;
  switch (value.kind) {
    case 'add':
      return { kind: 'add' };
    case 'noop':
      return str(value.existing) ? { kind: 'noop', existing: value.existing } : null;
    case 'extend':
      return str(value.target) ? { kind: 'extend', target: value.target } : null;
    case 'update':
      return str(value.target) ? { kind: 'update', target: value.target } : null;
    case 'historical':
      return str(value.current) ? { kind: 'historical', current: value.current } : null;
    case 'supersede': {
      const changes = parseChanges(value.changes);
      if (!str(value.target) || !changes || typeof value.explicit !== 'boolean') return null;
      return { kind: 'supersede', target: value.target, changes, explicit: value.explicit };
    }
    default:
      return null;
  }
}

function parseFact(value: unknown): RawFact | null {
  if (!isRecord(value) || value.kind !== 'fact' || !str(value.class) || !isRecord(value.properties)) return null;
  const subject = value.subject;
  if (subject !== null && subject !== undefined && !(isRecord(subject) && str(subject.id))) return null;
  return {
    kind: 'fact',
    class: value.class,
    subject: isRecord(subject) && str(subject.id) ? { id: subject.id, ...(str(subject.class) ? { class: subject.class } : {}) } : null,
    properties: value.properties,
    valid_from: str(value.valid_from) ? value.valid_from : null,
  };
}

function parseAction(value: unknown): SuggestionAction | null {
  if (!isRecord(value) || !str(value.kind)) return null;
  const v = value;
  switch (v.kind) {
    case 'remember': {
      const decision = parseDecision(v.decision);
      const content =
        isRecord(v.content) && v.content.kind === 'note' && str(v.content.text)
          ? { kind: 'note' as const, text: v.content.text }
          : parseFact(v.content);
      const decidedBy = v.decided_by;
      if (!decision || !content || !str(v.text) || !str(v.evidence)) return null;
      if (decidedBy !== 'rule' && decidedBy !== 'model' && decidedBy !== 'undecided') return null;
      return {
        kind: 'remember',
        content,
        text: v.text,
        decision,
        decided_by: decidedBy,
        target_text: str(v.target_text) ? v.target_text : null,
        evidence: v.evidence,
      };
    }
    case 'revise': {
      const content = parseFact(v.content);
      if (!content || !str(v.target) || !str(v.target_text) || !str(v.text) || !str(v.reason)) return null;
      return { kind: 'revise', target: v.target, target_text: v.target_text, content, text: v.text, reason: v.reason };
    }
    case 'link':
      if (!str(v.a) || !str(v.b) || !str(v.a_text) || !str(v.b_text) || !str(v.reason)) return null;
      return { kind: 'link', a: v.a, b: v.b, a_text: v.a_text, b_text: v.b_text, reason: v.reason };
    case 'resolve': {
      const changes = parseChanges(v.changes);
      if (!changes || !str(v.keep) || !str(v.keep_text) || !str(v.retire) || !str(v.retire_text)) return null;
      return { kind: 'resolve', keep: v.keep, keep_text: v.keep_text, retire: v.retire, retire_text: v.retire_text, changes };
    }
    case 'archive':
      if (!str(v.target) || !str(v.text) || typeof v.strength !== 'number') return null;
      return { kind: 'archive', target: v.target, text: v.text, strength: v.strength };
    default:
      return null;
  }
}

/** Validate one suggestion from the backend; `null` when malformed. */
export function parseSuggestion(value: unknown): Suggestion | null {
  if (!isRecord(value)) return null;
  const v = value;
  if (!str(v.id) || !str(v.createdAt)) return null;
  if (!STATUSES.includes(v.status as SuggestionStatus) || !KINDS.includes(v.kind as SuggestionKind)) return null;
  if (!(ORIGINS as readonly unknown[]).includes(v.origin)) return null;
  if (typeof v.confidence !== 'number' || !Number.isFinite(v.confidence)) return null;
  if (!Array.isArray(v.sensitive) || v.sensitive.some(r => !SENSITIVE.includes(r as SensitiveReason))) return null;
  const action = parseAction(v.action);
  if (!action || action.kind !== v.kind) return null;
  const optional = (x: unknown): string | null | undefined => (x === null || x === undefined ? null : str(x) ? x : undefined);
  const conversationId = optional(v.conversationId);
  const turnId = optional(v.turnId);
  const error = optional(v.error);
  const decidedAt = optional(v.decidedAt);
  if ([conversationId, turnId, error, decidedAt].some(x => x === undefined)) return null;
  return {
    id: v.id,
    kind: v.kind as SuggestionKind,
    origin: v.origin as Suggestion['origin'],
    status: v.status as SuggestionStatus,
    action,
    confidence: v.confidence,
    sensitive: v.sensitive as SensitiveReason[],
    conversationId: conversationId ?? null,
    turnId: turnId ?? null,
    // Builds before workspaces did not send a scope: everything was global then.
    scope: str(v.scope) && v.scope.length > 0 ? v.scope : 'global',
    error: error ?? null,
    undoable: v.undoable === true,
    createdAt: v.createdAt,
    decidedAt: decidedAt ?? null,
  };
}

/** Parse a list, dropping malformed entries. */
export function parseSuggestions(value: unknown): Suggestion[] {
  if (!Array.isArray(value)) return [];
  return value.map(parseSuggestion).filter((s): s is Suggestion => s !== null);
}

/** The memory text a suggestion would store or change. */
export function suggestionText(s: Suggestion): string {
  const a = s.action;
  switch (a.kind) {
    case 'remember':
    case 'revise':
      return a.text;
    case 'link':
      return `${a.a_text} ↔ ${a.b_text}`;
    case 'resolve':
      return a.keep_text;
    case 'archive':
      return a.text;
  }
}

function changeList(changes: ValueChange[]): string {
  return changes.map(c => `${c.label} ${c.from} → ${c.to}`).join('; ');
}

/** One line saying what accepting does ("Replaces: lives in Delhi → Pune"). */
export function describeDecision(s: Suggestion): string {
  const a = s.action;
  switch (a.kind) {
    case 'remember': {
      const d = a.decision;
      const target = a.target_text ? `“${a.target_text}”` : 'an existing memory';
      switch (d.kind) {
        case 'add':
          return a.decided_by === 'undecided'
            ? 'New memory — it may overlap with one you have; check before accepting'
            : 'New memory';
        case 'noop':
          return `Already remembered: ${target} — accepting strengthens it`;
        case 'extend':
          return `Adds to ${target}`;
        case 'supersede':
          return d.changes.length > 0
            ? `${d.explicit ? 'Replaces' : 'Supersedes'}: ${changeList(d.changes)} (the old value is kept in history)`
            : `Replaces ${target} (kept in history)`;
        case 'update':
          return `Refines ${target} (the old version is kept in history)`;
        case 'historical':
          return `Older than ${target}: kept as history only`;
      }
      return 'New memory';
    }
    case 'revise':
      return `New version of “${a.target_text}” — ${a.reason}`;
    case 'link':
      return `Links two memories about the same thing`;
    case 'resolve':
      return `Contradiction: ${changeList(a.changes)} — keeps the newer, closes “${a.retire_text}”`;
    case 'archive':
      return `Faded and unused: archive it (it stays in history and can be restored)`;
  }
}

const SENSITIVE_LABELS: Record<SensitiveReason, string> = {
  health: 'health',
  financial_identifier: 'financial identifier',
  credential: 'password or secret',
  third_party_personal: 'about someone else',
};

/** "Sensitive: health, about someone else" — empty when not sensitive. */
export function sensitiveLabel(reasons: readonly SensitiveReason[]): string {
  return reasons.length === 0 ? '' : `Sensitive: ${reasons.map(r => SENSITIVE_LABELS[r]).join(', ')}`;
}

/** Text fields of a suggestion the user can edit before accepting. */
export function editableSuggestionFields(s: Suggestion): { name: string; value: string }[] {
  const a = s.action;
  if (a.kind !== 'remember' && a.kind !== 'revise') return [];
  const content = a.content;
  if (content.kind === 'note') return [{ name: 'noteText', value: content.text }];
  return Object.entries(content.properties)
    .filter(([, v]) => typeof v === 'string')
    .map(([name, v]) => ({ name, value: v as string }));
}

/** The content to store after the user edited `draft` (field name → text); `null` when
 * nothing would be left. */
export function contentFromSuggestionEdit(s: Suggestion, draft: Record<string, string>): MemoryContent | null {
  const a = s.action;
  if (a.kind !== 'remember' && a.kind !== 'revise') return null;
  const content = a.content;
  if (content.kind === 'note') {
    const text = (draft.noteText ?? content.text).trim();
    return text ? { kind: 'note', text } : null;
  }
  const properties: Record<string, unknown> = { ...content.properties };
  for (const [name, value] of Object.entries(draft)) {
    if (typeof properties[name] !== 'string') continue;
    const trimmed = value.trim();
    if (!trimmed) return null;
    properties[name] = trimmed;
  }
  return { kind: 'fact', class: content.class, subject: content.subject ? { id: content.subject.id } : null, properties };
}

/** Pending suggestions that can be accepted together (not sensitive). */
export function batchAcceptable(suggestions: readonly Suggestion[]): Suggestion[] {
  return suggestions.filter(s => s.status === 'pending' && s.sensitive.length === 0);
}

export interface LearnStatus {
  mode: LearnMode;
  killSwitch: boolean;
  available: boolean;
  unavailableReason: string | null;
  model: string | null;
  pending: number;
  usage: { day: string; llmCalls: number; inputChars: number; outputChars: number; proposals: number; invalid: number; refused: number };
  caps: { maxCallsPerDay: number; maxInputCharsPerDay: number; maxProposalsPerDay: number };
  lastConsolidation: string | null;
}

/** Validate the learning status; `null` when malformed. */
export function parseLearnStatus(value: unknown): LearnStatus | null {
  if (!isRecord(value) || !isRecord(value.usage) || !isRecord(value.caps)) return null;
  const v = value;
  if (v.mode !== 'off' && v.mode !== 'ask' && v.mode !== 'auto') return null;
  if (typeof v.killSwitch !== 'boolean' || typeof v.available !== 'boolean' || typeof v.pending !== 'number') return null;
  const u = v.usage as Record<string, unknown>;
  const c = v.caps as Record<string, unknown>;
  const nums = (r: Record<string, unknown>, keys: string[]) => keys.every(k => typeof r[k] === 'number');
  if (!nums(u, ['llmCalls', 'inputChars', 'outputChars', 'proposals', 'invalid', 'refused']) || !str(u.day)) return null;
  if (!nums(c, ['maxCallsPerDay', 'maxInputCharsPerDay', 'maxProposalsPerDay'])) return null;
  return {
    mode: v.mode,
    killSwitch: v.killSwitch,
    available: v.available,
    unavailableReason: str(v.unavailableReason) ? v.unavailableReason : null,
    model: str(v.model) ? v.model : null,
    pending: v.pending,
    usage: u as unknown as LearnStatus['usage'],
    caps: c as unknown as LearnStatus['caps'],
    lastConsolidation: str(v.lastConsolidation) ? v.lastConsolidation : null,
  };
}

/** The badge text near the composer, or `null` when nothing waits. */
export function badgeText(pending: number): string | null {
  if (!Number.isFinite(pending) || pending <= 0) return null;
  return pending === 1 ? '1 suggested memory' : `${pending > 99 ? '99+' : pending} suggested memories`;
}
