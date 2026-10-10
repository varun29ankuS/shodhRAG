/**
 * ```derivation blocks: parsing, how each step is supported, the steps
 * around one step, and the focus target of a step.
 *   node --experimental-strip-types --test app/tests/visualDerivation.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { MAX_STEPS, parseDerivationBlock, stepContext, stepSupport, unsupportedSteps } from '../src/features/ask/visual/derivation.ts';
import { derivationStepTarget } from '../src/features/focus/targets.ts';
import { buildContextBlock, contextLabel } from '../src/features/focus/contextBlock.ts';
import { readTarget } from '../src/features/focus/threadStore.ts';

const SOURCE = JSON.stringify({
  title: 'Chunkwise delta rule',
  steps: [
    { latex: '$$S_t = S_{t-1} + \\beta_t (v_t - S_{t-1} k_t) k_t^\\top$$', justification: 'The delta rule update [2].' },
    { latex: 'S_t = S_{t-1}(I - \\beta_t k_t k_t^\\top) + \\beta_t v_t k_t^\\top', justification: 'Algebra: collect the S_{t-1} terms.' },
    { latex: 'S_t = \\sum_i \\beta_i v_i k_i^\\top \\prod_{j>i} (I - \\beta_j k_j k_j^\\top)', justification: 'Unroll the recursion', cites: [3, '4'] },
    { tex: 'P = I - W', why: 'It is well known.' },
  ],
});

test('steps are read with their LaTeX, reasons and cited passages', () => {
  const r = parseDerivationBlock(SOURCE);
  assert.ok(r.ok);
  if (!r.ok) return;
  const d = r.derivation;
  assert.equal(d.title, 'Chunkwise delta rule');
  assert.equal(d.steps.length, 4);
  // Dollar delimiters are not part of the LaTeX.
  assert.ok(d.steps[0].latex.startsWith('S_t = S_{t-1} +'));
  assert.deepEqual(d.steps[0].cites, [2]);
  assert.equal(d.steps[0].support, 'cited');
  assert.equal(d.steps[1].support, 'algebra');
  assert.deepEqual(d.steps[2].cites, [3, 4]);
  assert.equal(d.steps[3].latex, 'P = I - W');
  assert.equal(d.steps[3].support, 'unsupported');
  assert.deepEqual(unsupportedSteps(d), [3]);
  assert.deepEqual(stepContext(d, 0), { previous: null, next: d.steps[1].latex });
  assert.deepEqual(stepContext(d, 3), { previous: d.steps[2].latex, next: null });
});

test('malformed derivations are refused with a reason', () => {
  assert.equal(parseDerivationBlock('nope').ok, false);
  assert.equal(parseDerivationBlock('{"steps": []}').ok, false);
  assert.equal(parseDerivationBlock('{"steps": [{"justification": "x"}]}').ok, false);
  assert.equal(parseDerivationBlock(JSON.stringify({ steps: Array.from({ length: MAX_STEPS + 1 }, () => ({ latex: 'x' })) })).ok, false);
  // A bare list of steps is accepted.
  assert.equal(parseDerivationBlock('[{"latex": "a = b", "justification": "by definition"}]').ok, true);
  assert.equal(stepSupport('substitute (3) into (2)', []), 'algebra');
  assert.equal(stepSupport('obvious', []), 'unsupported');
});

test('a step opens with its neighbours, kept with the thread and sent as context', () => {
  const r = parseDerivationBlock(SOURCE);
  if (!r.ok) throw new Error('fixture');
  const d = r.derivation;
  const around = stepContext(d, 1);
  const target = derivationStepTarget({
    title: d.title,
    index: 1,
    total: d.steps.length,
    latex: d.steps[1].latex,
    justification: d.steps[1].justification,
    previous: around.previous,
    next: around.next,
    symbols: [
      { symbol: '\\beta_t', meaning: 'writing strength', definedAt: null },
      { symbol: '\\gamma', meaning: 'not in this step', definedAt: null },
    ],
  });
  assert.ok(target && target.kind === 'derivation_step');
  if (!target || target.kind !== 'derivation_step') return;
  assert.equal(target.label, 'Step 2 of 4 · Chunkwise delta rule');
  // Only the symbols that occur in the step are kept.
  assert.deepEqual(target.symbols?.map(s => s.symbol), ['\\beta_t']);
  assert.deepEqual(readTarget(JSON.parse(JSON.stringify(target))), target);
  const block = buildContextBlock(target);
  assert.match(block, /the step before \(LaTeX\)/);
  assert.match(block, /justification given/);
  assert.match(block, /the step after \(LaTeX\)/);
  assert.match(block, /\\beta_t: writing strength/);
  assert.equal(contextLabel(target), 'step 2 with its neighbours and justification');
  assert.equal(derivationStepTarget({ ...target, index: 4 }), null);
  assert.equal(readTarget({ ...target, index: 7 }), null);
});
