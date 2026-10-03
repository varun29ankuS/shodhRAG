/**
 * Where the agent asked the UI to go (`navigated` events with a target).
 *
 * `navigationFromEvent` validates the event (unknown target kinds or
 * malformed fields are dropped, keeping the plain view switch), and the
 * pending-target slot hands each target to the view that owns it: the view
 * may mount after the event (the tab switches first), so it takes the
 * target on mount and also listens for later ones.
 *
 * Pure apart from the module-level slot; type-only imports so Node's
 * strip-types test runner can load it.
 */

import type { NavigatedEvent, NavigationTarget } from './events';

export type TargetKind = NavigationTarget['kind'];

export interface AppNavigation {
  /** View id from the event (normalised by the caller). */
  view: string;
  focus: string | null;
  target: NavigationTarget | null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function optString(value: unknown): string | null {
  return typeof value === 'string' && value.trim().length > 0 ? value : null;
}

function reqString(value: unknown): string | null {
  return optString(value);
}

/** A valid target, or null for anything malformed or unknown. */
export function parseTarget(value: unknown): NavigationTarget | null {
  if (!isRecord(value)) return null;
  switch (value.kind) {
    case 'document': {
      const path = reqString(value.path);
      if (!path) return null;
      const page = typeof value.page === 'number' && Number.isInteger(value.page) && value.page > 0 ? value.page : null;
      return { kind: 'document', path, page, passage: optString(value.passage) };
    }
    case 'calendar': {
      const date = optString(value.date);
      const taskId = optString(value.taskId);
      const eventId = optString(value.eventId);
      if (!date && !taskId && !eventId) return null;
      return { kind: 'calendar', date: date && /^\d{4}-\d{2}-\d{2}$/.test(date) ? date : null, taskId, eventId };
    }
    case 'conversation': {
      const conversationId = reqString(value.conversationId);
      return conversationId ? { kind: 'conversation', conversationId } : null;
    }
    case 'audit': {
      const types = Array.isArray(value.types) ? value.types.filter((t): t is string => typeof t === 'string') : [];
      return {
        kind: 'audit',
        types,
        tool: optString(value.tool),
        from: optString(value.from),
        to: optString(value.to),
        text: optString(value.text),
      };
    }
    case 'source': {
      const sourceId = reqString(value.sourceId);
      return sourceId ? { kind: 'source', sourceId } : null;
    }
    case 'visual': {
      const visualId = reqString(value.visualId);
      const version = typeof value.version === 'number' && Number.isInteger(value.version) && value.version > 0 ? value.version : null;
      return visualId ? { kind: 'visual', visualId, version } : null;
    }
    default:
      return null;
  }
}

/** The navigation a `navigated` event asks for. */
export function navigationFromEvent(event: NavigatedEvent): AppNavigation {
  return {
    view: event.view,
    focus: optString(event.focus),
    // Events from older builds have no `target` key at all.
    target: parseTarget((event as { target?: unknown }).target ?? null),
  };
}

type Listener = (target: NavigationTarget) => void;

const pending = new Map<TargetKind, NavigationTarget>();
const listeners = new Set<Listener>();

/** Hand `target` to its view: listeners now, or the view when it mounts. */
export function publishTarget(target: NavigationTarget): void {
  pending.set(target.kind, target);
  for (const listener of listeners) listener(target);
}

/** Take (and clear) the pending target of `kind`, if any. */
export function takeTarget<K extends TargetKind>(kind: K): Extract<NavigationTarget, { kind: K }> | null {
  const target = pending.get(kind);
  if (!target) return null;
  pending.delete(kind);
  return target as Extract<NavigationTarget, { kind: K }>;
}

/** The pending target of `kind`, left pending for its owner. */
export function peekTarget<K extends TargetKind>(kind: K): Extract<NavigationTarget, { kind: K }> | null {
  const target = pending.get(kind);
  return target ? (target as Extract<NavigationTarget, { kind: K }>) : null;
}

/** Listen for targets; returns the unsubscribe function. */
export function subscribeTargets(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
