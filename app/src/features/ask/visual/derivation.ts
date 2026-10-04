/**
 * Step-by-step derivations written by the agent as ```derivation fences:
 *
 *   { "title": "Chunkwise form of the delta rule",
 *     "steps": [ { "latex": "S_t = S_{t-1}(I - \\beta_t k_t k_t^\\top) + \\beta_t v_t k_t^\\top",
 *                  "justification": "Expand the update of Eq. (1) [2]", "cites": [2] }, … ] }
 *
 * Every step shows one transformation and why it holds. A justification
 * must cite a passage (`cites` or `[n]` in its text) or say it is algebra;
 * steps that do neither are flagged so the reader knows the step is
 * unsupported.
 *
 * Pure module, unit-tested with Node (`app/tests/visualDerivation.test.ts`).
 */

import { parseCitationMarkers } from '../../agent/grounding.ts';

export const MAX_STEPS = 40;
const MAX_LATEX_CHARS = 2_000;
const MAX_JUSTIFICATION_CHARS = 800;
const MAX_TITLE_CHARS = 160;
const MAX_CITES = 12;

export type StepSupport = 'cited' | 'algebra' | 'unsupported';

export interface DerivationStep {
  latex: string;
  justification: string;
  /** Passage numbers the step relies on (from `cites` and `[n]` in the text). */
  cites: number[];
  support: StepSupport;
}

export interface Derivation {
  title: string;
  steps: DerivationStep[];
}

export type DerivationParseResult = { ok: true; derivation: Derivation } | { ok: false; error: string };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function text(value: unknown, max: number): string {
  return typeof value === 'string' ? Array.from(value.replace(/\s+/g, ' ').trim()).slice(0, max).join('') : '';
}

/** Words that say a step needs no source: algebra, a definition, a known identity. */
const SELF_EVIDENT = /\b(?:algebra(?:ic)?|rearrang\w*|simplif\w*|substitut\w*|expand\w*|factor\w*|by definition|definition of|identity|distribut\w*|cancel\w*|collect\w*)\b/i;

/** Whether a step is cited, self-evident algebra, or unsupported. */
export function stepSupport(justification: string, cites: readonly number[]): StepSupport {
  if (cites.length > 0) return 'cited';
  if (SELF_EVIDENT.test(justification)) return 'algebra';
  return 'unsupported';
}

function readCites(value: unknown, justification: string): number[] {
  const list = Array.isArray(value) ? value : value === undefined || value === null ? [] : [value];
  const out = new Set<number>();
  for (const v of list) {
    const n = typeof v === 'number' ? v : typeof v === 'string' ? Number(v.replace(/[[\]\s]/g, '')) : NaN;
    if (Number.isInteger(n) && n > 0) out.add(n);
  }
  for (const marker of parseCitationMarkers(justification)) for (const n of marker.numbers) out.add(n);
  return Array.from(out).slice(0, MAX_CITES);
}

export function parseDerivationBlock(source: string): DerivationParseResult {
  let parsed: unknown;
  try {
    parsed = JSON.parse(source);
  } catch {
    return { ok: false, error: 'The derivation is not valid JSON.' };
  }
  const root = Array.isArray(parsed) ? { steps: parsed } : parsed;
  if (!isRecord(root)) return { ok: false, error: 'The derivation must be a JSON object with "steps".' };
  if (!Array.isArray(root.steps) || root.steps.length === 0) return { ok: false, error: 'The derivation needs a non-empty "steps" list.' };
  if (root.steps.length > MAX_STEPS) return { ok: false, error: `At most ${MAX_STEPS} steps are supported.` };
  const steps: DerivationStep[] = [];
  for (let i = 0; i < root.steps.length; i++) {
    const step = root.steps[i];
    if (!isRecord(step)) return { ok: false, error: `Step ${i + 1} must be an object.` };
    const rawLatex = typeof step.latex === 'string' ? step.latex : typeof step.tex === 'string' ? step.tex : '';
    const latex = rawLatex.trim().replace(/^\$\$?|\$\$?$/g, '').trim();
    if (!latex) return { ok: false, error: `Step ${i + 1} needs "latex".` };
    if (latex.length > MAX_LATEX_CHARS) return { ok: false, error: `Step ${i + 1} is longer than ${MAX_LATEX_CHARS} characters.` };
    const justification = text(step.justification ?? step.why ?? step.reason, MAX_JUSTIFICATION_CHARS);
    const cites = readCites(step.cites ?? step.cite, justification);
    steps.push({ latex, justification, cites, support: stepSupport(justification, cites) });
  }
  return { ok: true, derivation: { title: text(root.title, MAX_TITLE_CHARS), steps } };
}

/** The steps around step `index`, for a question about that step. */
export function stepContext(derivation: Derivation, index: number): { previous: string | null; next: string | null } {
  return {
    previous: index > 0 ? derivation.steps[index - 1]?.latex ?? null : null,
    next: derivation.steps[index + 1]?.latex ?? null,
  };
}

/** Steps that neither cite a passage nor say they are algebra. */
export function unsupportedSteps(derivation: Derivation): number[] {
  return derivation.steps.flatMap((s, i) => (s.support === 'unsupported' ? [i] : []));
}
