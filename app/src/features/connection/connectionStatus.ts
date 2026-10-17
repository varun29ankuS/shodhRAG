/**
 * The model status in the sidebar (`connection_health.rs`): which model
 * answers, through which provider, how it is connected, and whether it works.
 * Readers check the backend's data before use; the text and colour mapping
 * is pure so it is unit-tested with Node (`app/tests/connectionStatus.test.ts`).
 */
import type { ActiveModel, ProviderId } from '../modelPicker/modelTypes.ts';
import { PROVIDER_LABELS, isProviderId, readModelRef } from '../modelPicker/modelTypes.ts';
import { friendlyModel } from '../agent/format.ts';

/** Emitted by the backend with a snapshot whenever a status changes. */
export const CONNECTION_STATUS_EVENT = 'connection-status-changed';

export type Health = 'works' | 'rejected' | 'limited' | 'unchecked' | 'unreachable';
export type ConnectionSource = 'saved' | 'environment' | 'subscription' | 'local';
/** Where a status was learned: a check (no chat request) or an answer. */
export type Evidence = 'check' | 'answer';

export interface ProviderStatus {
  provider: ProviderId;
  label: string;
  source: ConnectionSource | null;
  health: Health;
  /** The provider's message (never containing the key), or why. */
  message: string | null;
  httpStatus: number | null;
  /** For `limited`: out of credit or quota rather than rate-limited. */
  quota: boolean;
  /** For `limited`: when the limit resets (Unix ms), when the provider said. */
  resetAtMs: number | null;
  checkedAtMs: number | null;
  learnedFrom: Evidence | null;
  /** The last characters of the key in use, masked (`…a3f9`). */
  keyHint: string | null;
  /** The provider's environment variable holding a key, if any. */
  envVar: string | null;
  checking: boolean;
}

export interface ConnectionSnapshot {
  active: ActiveModel | null;
  providers: ProviderStatus[];
  localOnly: boolean;
}

const HEALTHS: readonly Health[] = ['works', 'rejected', 'limited', 'unchecked', 'unreachable'];
const SOURCES: readonly ConnectionSource[] = ['saved', 'environment', 'subscription', 'local'];
const EVIDENCE: readonly Evidence[] = ['check', 'answer'];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function oneOf<T extends string>(values: readonly T[], value: unknown): T | null {
  return typeof value === 'string' && (values as readonly string[]).includes(value) ? (value as T) : null;
}

function text(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value : null;
}

function whole(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? Math.floor(value) : null;
}

/** The last characters a key hint may show: `…` and at most four more. */
function safeHint(value: unknown): string | null {
  const hint = text(value);
  if (!hint || !hint.startsWith('…')) return null;
  return `…${hint.slice(1).slice(-4)}`;
}

export function readProviderStatus(value: unknown): ProviderStatus | null {
  if (!isRecord(value) || !isProviderId(value.provider)) return null;
  return {
    provider: value.provider,
    label: text(value.label) ?? PROVIDER_LABELS[value.provider],
    source: oneOf(SOURCES, value.source),
    health: oneOf(HEALTHS, value.health) ?? 'unchecked',
    message: text(value.message),
    httpStatus: whole(value.httpStatus),
    quota: value.quota === true,
    resetAtMs: whole(value.resetAtMs),
    checkedAtMs: whole(value.checkedAtMs),
    learnedFrom: oneOf(EVIDENCE, value.learnedFrom),
    keyHint: safeHint(value.keyHint),
    envVar: text(value.envVar),
    checking: value.checking === true,
  };
}

/** A snapshot from `connection_status` or the change event; null when malformed. */
export function readSnapshot(value: unknown): ConnectionSnapshot | null {
  if (!isRecord(value) || !Array.isArray(value.providers)) return null;
  const model = isRecord(value.active) ? readModelRef(value.active.model) : null;
  const source = isRecord(value.active) ? oneOf(['session', 'environment', 'settings'] as const, value.active.source) : null;
  return {
    active: model && source ? { model, source } : null,
    providers: value.providers.map(readProviderStatus).filter((p): p is ProviderStatus => p !== null),
    localOnly: value.localOnly === true,
  };
}

export type Dot = 'green' | 'red' | 'amber' | 'grey';

/** Green works, red rejected, amber limited, grey not checked or unreachable. */
export function healthDot(health: Health): Dot {
  switch (health) {
    case 'works':
      return 'green';
    case 'rejected':
      return 'red';
    case 'limited':
      return 'amber';
    case 'unchecked':
    case 'unreachable':
      return 'grey';
  }
}

export const DOT_CLASS: Record<Dot, string> = {
  green: 'bg-shodh-success',
  red: 'bg-shodh-error',
  amber: 'bg-shodh-warning',
  grey: 'bg-shodh-text-faint',
};

/** How a time reads in the status (the app passes the locale's time format). */
export type TimeFormat = (ms: number) => string;

export const localTime: TimeFormat = ms => {
  const at = new Date(ms);
  const sameDay = at.toDateString() === new Date().toDateString();
  const time = at.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
  return sameDay ? time : `${at.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })}, ${time}`;
};

/** One line on how the provider is doing. */
export function healthText(status: ProviderStatus, formatTime: TimeFormat = localTime): string {
  const said = status.message ? `: ${status.message}` : '';
  switch (status.health) {
    case 'works':
      return 'Working';
    case 'rejected':
      if (status.source === 'subscription') return status.message ?? 'Sign-in not accepted';
      return `Key rejected${status.httpStatus ? ` (${status.httpStatus})` : ''}${said}`;
    case 'limited': {
      const what = status.quota ? 'Out of credit or daily quota' : 'Rate-limited';
      const reset = status.resetAtMs !== null ? `, resets ${formatTime(status.resetAtMs)}` : '';
      return `${what}${reset}${said}`;
    }
    case 'unreachable':
      return `Can't reach ${status.label}${said}`;
    case 'unchecked':
      return status.message ?? 'Not checked yet';
  }
}

/** How the provider is connected. */
export function sourceText(status: ProviderStatus): string {
  switch (status.source) {
    case 'saved':
      return 'Key saved in the app';
    case 'environment':
      return status.envVar ? `Key from the ${status.envVar} environment variable` : 'Key from an environment variable';
    case 'subscription':
      return 'Subscription sign-in';
    case 'local':
      return 'Server on this computer';
    case null:
      return 'Not connected';
  }
}

/** Short form for the pill. */
export function sourceShort(source: ConnectionSource | null): string {
  switch (source) {
    case 'saved':
      return 'saved key';
    case 'environment':
      return 'env key';
    case 'subscription':
      return 'sign-in';
    case 'local':
      return 'local';
    case null:
      return 'not connected';
  }
}

/** Which key is in use when an environment variable is set too; null when there is nothing to say. */
export function keyOriginNote(status: ProviderStatus): string | null {
  if (!status.envVar) return null;
  if (status.source === 'saved') {
    return `Using the key saved in the app. ${status.envVar} is also set, but it is not used.`;
  }
  if (status.source === 'environment') {
    return `Using the key from ${status.envVar}. No key is saved in the app.`;
  }
  return null;
}

/** When the status was learned: "just now", "4 min ago", or the time. */
export function checkedText(status: ProviderStatus, nowMs: number, formatTime: TimeFormat = localTime): string {
  if (status.checking) return 'Checking…';
  if (status.checkedAtMs === null) return 'Not checked yet';
  const how = status.learnedFrom === 'answer' ? 'Seen in a chat' : 'Checked';
  const ago = Math.max(0, nowMs - status.checkedAtMs);
  if (ago < 60_000) return `${how} just now`;
  if (ago < 60 * 60_000) return `${how} ${Math.floor(ago / 60_000)} min ago`;
  return `${how} at ${formatTime(status.checkedAtMs)}`;
}

/** What the fix button of a provider says (it opens that provider's connect UI). */
export function fixLabel(status: ProviderStatus): string {
  return status.source === 'subscription' ? 'Fix sign-in' : status.source === 'local' ? 'Open settings' : 'Fix key';
}

/** The status of the provider that answers, if listed. */
export function activeStatus(snapshot: ConnectionSnapshot): ProviderStatus | null {
  const provider = snapshot.active?.model.provider;
  return provider ? snapshot.providers.find(p => p.provider === provider) ?? null : null;
}

export interface PillView {
  /** The model's friendly name (`friendlyModel`, as the answer's status line shows it), or "No model chosen". */
  model: string;
  /** "OpenRouter · saved key", or null without a model. */
  via: string | null;
  dot: Dot;
  /** The full status, for the tooltip and screen readers. */
  description: string;
}

function sentence(line: string): string {
  return /[.!?…]$/.test(line) ? line : `${line}.`;
}

/** What the sidebar's model status shows (one wording wherever the status appears). */
export function pillView(snapshot: ConnectionSnapshot | null, formatTime: TimeFormat = localTime): PillView {
  if (!snapshot) return { model: 'Model', via: null, dot: 'grey', description: 'Connection status loading' };
  const active = snapshot.active;
  if (!active) {
    return { model: 'No model chosen', via: null, dot: 'grey', description: 'No model is chosen. Choose one to answer.' };
  }
  const status = activeStatus(snapshot);
  const label = status?.label ?? PROVIDER_LABELS[active.model.provider];
  const model = friendlyModel(active.model.model);
  const via = `${label} · ${sourceShort(status?.source ?? null)}`;
  const health = status ? healthText(status, formatTime) : 'Not connected';
  return {
    model,
    via,
    dot: status ? healthDot(status.health) : 'grey',
    description: `${active.model.model} through ${label} (${status ? sourceText(status) : 'not connected'}). ${sentence(health)}`,
  };
}
