/**
 * Refining a gallery visual: the request sent through the side-thread path
 * with the current spec, and the parsing of the answer, which must hold
 * exactly one block of the same kind (anything else is rejected with a
 * message the user can act on).
 *
 * Pure module, unit-tested with Node (`app/tests/visualGallery.test.ts`).
 */

import type { FocusParamValue } from '../focus/focusTypes.ts';
import { fenced } from '../focus/contextBlock.ts';
import { extractVisualBlocks, KIND_FENCE, KIND_NOUN, MAX_VISUAL_SOURCE_CHARS, normalizeSource } from './extract.ts';
import type { VisualKind } from './extract.ts';

/** Longest change request the user can type. */
export const MAX_REFINE_INSTRUCTION_CHARS = 1_000;

export interface RefineInput {
  kind: VisualKind;
  title: string;
  source: string;
  /** Slider positions of a plot or simulation when the request was made. */
  values: readonly FocusParamValue[];
  instruction: string;
}

function noun(kind: VisualKind): string {
  return KIND_NOUN[kind].toLowerCase();
}

/** "a diagram", "an equation". */
function withArticle(word: string): string {
  return `${/^[aeiou]/i.test(word) ? 'an' : 'a'} ${word}`;
}

/** How the revised block must be written, per kind. */
function blockShape(kind: VisualKind): string {
  const fence = KIND_FENCE[kind];
  if (fence) return `one \`\`\`${fence} code block`;
  if (kind === 'equation') return 'one display equation written as $$ … $$ on its own lines';
  return 'one Markdown table (header row, |---| separator row, then the rows)';
}

/** The current visual as the request shows it. */
function currentBlock(input: RefineInput): string {
  const fence = KIND_FENCE[input.kind];
  if (fence) return fenced(input.source, fence);
  if (input.kind === 'equation') return `$$\n${input.source}\n$$`;
  return input.source;
}

/** The message that asks for a revised version. */
export function composeRefineRequest(input: RefineInput): string {
  const instruction = Array.from(input.instruction.trim()).slice(0, MAX_REFINE_INSTRUCTION_CHARS).join('');
  const sliders = input.values.length > 0 && (input.kind === 'plot' || input.kind === 'simulation')
    ? `\nThe sliders were at ${input.values.map(v => `${v.name} = ${v.value}`).join(', ')}; keep these as the starting values unless the change says otherwise.`
    : '';
  return [
    `Revise this ${noun(input.kind)} ("${input.title}") as asked.`,
    '',
    `Change requested: ${instruction}`,
    '',
    `Current ${noun(input.kind)}:`,
    currentBlock(input),
    sliders.trim(),
    '',
    `Reply with exactly ${blockShape(input.kind)} holding the complete revised ${noun(input.kind)}, `
      + 'self-contained and in the same format, followed by at most one sentence saying what changed. '
      + `Do not include any other ${noun(input.kind)}, diagram, chart, sketch, plot, simulation, display equation or table, `
      + 'do not search documents, and do not use tools.',
  ].filter((line, i, all) => line !== '' || all[i - 1] !== '').join('\n');
}

export type RefineResult =
  | { ok: true; source: string; note: string }
  | { ok: false; message: string };

/**
 * The revised source in `answer`: exactly one block of `kind` that draws.
 * `previous` (the version refined) is compared so an unchanged copy is
 * reported instead of saved as a new version.
 */
export function parseRefineResponse(answer: string, kind: VisualKind, previous: string): RefineResult {
  const blocks = extractVisualBlocks(answer);
  const same = blocks.filter(b => b.kind === kind);
  const want = noun(kind);
  if (same.length === 0) {
    const other = blocks[0];
    return {
      ok: false,
      message: other
        ? `The answer contained ${withArticle(noun(other.kind))}, not ${withArticle(want)}. Ask again, or say which ${want} to change.`
        : `The answer did not contain a revised ${want}. Try describing the change differently.`,
    };
  }
  if (same.length > 1) {
    return { ok: false, message: `The answer contained ${same.length} ${want}s; a refinement must return exactly one. Ask again for a single ${want}.` };
  }
  const block = same[0];
  if (block.problem) {
    return { ok: false, message: `The revised ${want} cannot be drawn: ${block.problem}` };
  }
  if (Array.from(block.source).length > MAX_VISUAL_SOURCE_CHARS) {
    return { ok: false, message: `The revised ${want} is larger than the gallery keeps.` };
  }
  if (normalizeSource(block.source) === normalizeSource(previous)) {
    return { ok: false, message: `The answer returned the ${want} unchanged. Try describing the change differently.` };
  }
  // The one sentence after the block, if any, describes the change.
  const note = answer
    .replace(/(`{3,}|~{3,})[\s\S]*?\1/g, '')
    .replace(/\$\$[\s\S]*?\$\$/g, '')
    .split('\n')
    .map(l => l.trim())
    .filter(l => l && !l.includes('|'))
    .join(' ')
    .slice(0, 300);
  return { ok: true, source: block.source, note };
}
