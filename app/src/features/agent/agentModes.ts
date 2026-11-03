/**
 * Agent modes: how much of an answer's work runs without asking (the Rust side is
 * `crates/shodh-rag/src/harness/modes.rs`, which decides). The user picks one per
 * conversation in the composer (Shift+Tab cycles); the default is in Settings. The
 * agent can only suggest a mode (`suggest_mode`), which shows as a button; it never
 * switches one.
 *
 * Pure (no React, no Tauri) so it is unit-tested directly with Node
 * (`app/tests/agentModes.test.ts`).
 */

export type AgentMode = 'ask' | 'acceptEdits' | 'plan' | 'auto';

export interface AgentModeInfo {
  id: AgentMode;
  /** The mode's name, as the picker shows it. */
  label: string;
  /** What runs without asking, in one plain line. */
  hint: string;
}

/** Every mode, in the order the picker lists them and Shift+Tab cycles them. */
export const AGENT_MODES: readonly AgentModeInfo[] = [
  { id: 'ask', label: 'Ask', hint: 'Reads and searches run by themselves; every change asks you first.' },
  {
    id: 'acceptEdits',
    label: 'Accept edits',
    hint: 'Edits in the project or workspace run by themselves; commands and anything that leaves this computer ask.',
  },
  { id: 'plan', label: 'Plan', hint: 'Only reads: the assistant looks into it and writes a plan. You approve it and pick the mode it runs in.' },
  {
    id: 'auto',
    label: 'Auto',
    hint: 'Edits, tests, builds and other low-risk commands run by themselves; risky commands and anything that leaves this computer ask.',
  },
];

/** What always asks, whatever the mode. */
export const MODE_FLOOR =
  'In every mode, pushes, pull requests, comments and merges on GitHub, deleting, downloads and memories ask you first.';

export const DEFAULT_AGENT_MODE: AgentMode = 'ask';

/** Whether `value` names a mode. */
export function isAgentMode(value: unknown): value is AgentMode {
  return AGENT_MODES.some(m => m.id === value);
}

/** The mode named `value`, or null (a mode this version does not know is never guessed). */
export function parseAgentMode(value: unknown): AgentMode | null {
  return isAgentMode(value) ? value : null;
}

export function agentModeInfo(mode: AgentMode): AgentModeInfo {
  return AGENT_MODES.find(m => m.id === mode) ?? AGENT_MODES[0];
}

/** The mode after `mode` (Shift+Tab), wrapping around. */
export function nextAgentMode(mode: AgentMode): AgentMode {
  const at = AGENT_MODES.findIndex(m => m.id === mode);
  return AGENT_MODES[(at + 1) % AGENT_MODES.length].id;
}

/** The modes a plan can be approved to run in: every mode that can change anything. */
export const PLAN_RUN_MODES: readonly AgentMode[] = ['acceptEdits', 'auto', 'ask'];

/** What is sent when the user approves a plan. */
export const PLAN_APPROVED_TEXT = 'The plan is approved. Go ahead with it.';

/** A mode the agent suggested in an answer (its `suggest_mode` step). */
export interface ModeSuggestion {
  mode: AgentMode;
  reason: string;
}

/** The parts of a transcript step this reads. */
interface StepLike {
  tool: string;
  status: string;
  detail: unknown;
}

/**
 * The agent's last mode suggestion in an answer's steps, if it is for another mode than
 * `current`. Only a finished `suggest_mode` step counts, and only a mode this version
 * knows: anything else in its detail is ignored.
 */
export function modeSuggestion(steps: readonly StepLike[], current: AgentMode): ModeSuggestion | null {
  for (let i = steps.length - 1; i >= 0; i--) {
    const step = steps[i];
    if (step.tool !== 'suggest_mode' || step.status !== 'done') continue;
    const detail = step.detail;
    if (typeof detail !== 'object' || detail === null) continue;
    const { suggestedMode, reason } = detail as { suggestedMode?: unknown; reason?: unknown };
    const mode = parseAgentMode(suggestedMode);
    if (!mode || mode === current) return null;
    return { mode, reason: typeof reason === 'string' ? reason.slice(0, 160) : '' };
  }
  return null;
}
