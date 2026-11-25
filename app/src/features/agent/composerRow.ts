/**
 * The composer's one row of controls, shared by Ask and the floating chat: the order of
 * its controls, the one height every control has, and the mode menu's two sections
 * (what the answer works on, and its permissions). Controls never wrap: at narrow
 * widths their labels truncate and the row keeps its height.
 *
 * Pure (no React, no Tauri) so it is unit-tested directly with Node
 * (`app/tests/composerRow.test.ts`).
 */

import { AGENT_MODES, DEFAULTABLE_MODES, DEFAULT_AGENT_MODE } from './agentModes.ts';
import type { AgentMode } from './agentModes';

/** Every control's height in the row, in CSS pixels. */
export const COMPOSER_CONTROL_PX = 32;

/** The height class every control in the row uses (`COMPOSER_CONTROL_PX`). */
export const COMPOSER_CONTROL_HEIGHT = 'h-8';

/** A square icon control in the row. */
export const COMPOSER_ICON_CONTROL = 'w-8 h-8';

/** The row itself: one line, never wrapping, as tall as its controls. */
export const COMPOSER_ROW_CLASS = 'flex flex-nowrap items-center gap-1.5 min-w-0 h-8';

/** The row's controls, left to right; Send (or Interrupt) is always last, at the far right. */
export const COMPOSER_ROW_ORDER = ['image', 'dictate', 'mode', 'model', 'send'] as const;
export type ComposerControl = typeof COMPOSER_ROW_ORDER[number];

/** The `data-testid` of each control (the probes over CDP select by these). */
export const COMPOSER_TEST_IDS: Record<ComposerControl | 'row' | 'stop' | 'steer', string> = {
  row: 'composer-row',
  image: 'composer-image',
  dictate: 'composer-dictate',
  mode: 'composer-mode',
  model: 'composer-model',
  send: 'composer-send',
  stop: 'composer-stop',
  steer: 'composer-steer',
};

/** Research or Code: the conversation's own mode (`Conversation.mode`). */
export type ConversationWork = 'research' | 'code';

/** What an answer works on, as the mode menu's first section lists it. */
export type WorkKind = 'research' | 'code' | 'creator';

export interface WorkInfo {
  id: WorkKind;
  label: string;
  /** One short line, for the item's tooltip. */
  short: string;
}

export const WORK_KINDS: readonly WorkInfo[] = [
  { id: 'research', label: 'Research', short: 'Your library and the web' },
  { id: 'code', label: 'Code', short: 'Your code folder' },
  { id: 'creator', label: 'Creator', short: 'Builds in a new folder' },
];

export function workInfo(kind: WorkKind): WorkInfo {
  return WORK_KINDS.find(w => w.id === kind) ?? WORK_KINDS[0];
}

/** The permissions the second section lists (Creator sets its own). */
export const PERMISSION_MODES: readonly AgentMode[] = DEFAULTABLE_MODES;

/** Why the permissions section is off in Creator (one short line). */
export const CREATOR_PERMISSIONS_NOTE = 'Creator sets its own';

/** What an answer works on: Creator is an agent mode; Research and Code are the conversation's. */
export function workKindOf(conversationMode: ConversationWork, agentMode: AgentMode | null): WorkKind {
  return agentMode === 'creator' ? 'creator' : conversationMode;
}

/** Whether the permissions section applies to `kind` (it shows either way, never hidden). */
export function permissionsApply(kind: WorkKind): boolean {
  return kind !== 'creator';
}

export function isWorkKind(value: unknown): value is WorkKind {
  return value === 'research' || value === 'code' || value === 'creator';
}

export function isPermissionMode(value: unknown): value is AgentMode {
  return typeof value === 'string' && (PERMISSION_MODES as readonly string[]).includes(value);
}

/** What changes on the conversation when the user picks `next` in the menu. */
export interface WorkChange {
  conversationMode?: ConversationWork;
  agentMode?: AgentMode;
}

/**
 * The conversation's change for picking `next`. Creator is an agent mode, so picking it
 * leaves Research/Code as it was; leaving Creator restores the permission the
 * conversation had before (`lastPermission`), else the default, else Ask first.
 */
export function workChange(
  next: WorkKind,
  current: { conversationMode: ConversationWork; agentMode: AgentMode | null },
  lastPermission: AgentMode | null,
  defaultMode: AgentMode | null,
): WorkChange {
  if (next === 'creator') return current.agentMode === 'creator' ? {} : { agentMode: 'creator' };
  const change: WorkChange = {};
  if (next !== current.conversationMode) change.conversationMode = next;
  if (current.agentMode === 'creator') {
    const restored = [lastPermission, defaultMode].find(isPermissionMode);
    change.agentMode = restored ?? DEFAULT_AGENT_MODE;
  }
  return change;
}

/** The mode menu trigger's text: the work, and the permission where it applies. */
export function modeTriggerText(kind: WorkKind, permission: AgentMode | null): { label: string; detail: string | null } {
  const label = workInfo(kind).label;
  if (!permissionsApply(kind) || permission === null) return { label, detail: null };
  return { label, detail: AGENT_MODES.find(m => m.id === permission)?.label ?? null };
}
