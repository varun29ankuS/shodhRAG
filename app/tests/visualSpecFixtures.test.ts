/**
 * The spec fixture cases shared with the Rust validator
 * (`crates/shodh-rag/src/visuals/fixtures/specs.json`): the renderer's readers
 * must give each case the verdict the backend gives it, so a revision the
 * backend saves always draws and one it refuses would not.
 *   node --experimental-strip-types --test app/tests/visualSpecFixtures.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { parseChartBlock } from '../src/features/ask/visual/chartSpec.ts';
import { parsePlotSpec } from '../src/features/ask/visual/plotSpec.ts';
import { parseSimulationSpec } from '../src/features/ask/visual/simulationSpec.ts';

interface Case {
  name: string;
  kind: 'chart' | 'plot' | 'simulation';
  spec?: unknown;
  source?: string;
  valid: boolean;
}

const cases = JSON.parse(
  readFileSync(new URL('../../crates/shodh-rag/src/visuals/fixtures/specs.json', import.meta.url), 'utf8'),
) as Case[];

function verdict(c: Case): { ok: boolean; error?: string } {
  const source = c.source ?? JSON.stringify(c.spec);
  const result = c.kind === 'chart' ? parseChartBlock(source) : c.kind === 'plot' ? parsePlotSpec(source) : parseSimulationSpec(source);
  return result.ok ? { ok: true } : { ok: false, error: result.error };
}

test('the fixture file has cases of every kind', () => {
  assert.ok(cases.length >= 40);
  for (const kind of ['chart', 'plot', 'simulation']) assert.ok(cases.some(c => c.kind === kind), kind);
  assert.ok(cases.some(c => c.valid) && cases.some(c => !c.valid));
});

for (const c of cases) {
  test(`${c.name} → ${c.valid ? 'draws' : 'refused'}`, () => {
    const v = verdict(c);
    assert.equal(v.ok, c.valid, v.error ?? 'drew');
  });
}
