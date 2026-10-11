/**
 * Deterministic mermaid repair: realistic broken diagrams get the expected
 * repair, valid diagrams come back unchanged, and the repair is idempotent.
 * (The fixtures were checked against the mermaid 11 parser: every broken
 * source is rejected and every expected repair parses.)
 *   node --experimental-strip-types --test app/tests/mermaidRepair.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { quoteLabel, repairMermaid } from '../src/features/ask/visual/mermaidRepair.ts';
import { BROKEN, VALID } from './mermaidFixtures.ts';

for (const fixture of BROKEN) {
  test(`repair: ${fixture.name}`, () => {
    const result = repairMermaid(fixture.source);
    assert.equal(result.source, fixture.repaired);
    assert.equal(result.changed, true);
    assert.ok(result.fixes.length > 0, 'names the fixes it applied');
    const again = repairMermaid(result.source);
    assert.equal(again.source, result.source, 'idempotent');
    assert.equal(again.changed, false);
    assert.deepEqual(again.fixes, []);
  });
}

test('repair: valid diagrams come back unchanged', () => {
  for (const source of VALID) {
    const result = repairMermaid(source);
    assert.equal(result.changed, false, source);
    assert.equal(result.source, source);
    assert.deepEqual(result.fixes, []);
  }
});

test('repair: fixes are reported by kind in a stable order', () => {
  const result = repairMermaid('flowchart TD\n  A[<b>Load</b> (CSV)] → B\n  graph TD\n  B -->');
  assert.deepEqual(result.fixes, ['duplicate-headers', 'html-tags', 'arrows', 'quoted-labels', 'empty-edges']);
  assert.equal(result.source, 'flowchart TD\n  A["Load (CSV)"] --> B\n  B');
});

test('repair: <br/> line breaks and generic types are kept', () => {
  assert.equal(repairMermaid('flowchart LR\n  A[one<br/>two] --> B').changed, false);
  assert.equal(repairMermaid('classDiagram\n  class Box~T~\n  Box : List<String> items').changed, false);
});

test('repair: Windows line endings are accepted', () => {
  const result = repairMermaid('flowchart TD\r\n  A[Start (init)] --> B\r\n');
  assert.equal(result.source, 'flowchart TD\n  A["Start (init)"] --> B\n');
});

test('repair: empty and headerless input is left alone', () => {
  assert.deepEqual(repairMermaid(''), { source: '', changed: false, fixes: [] });
  assert.deepEqual(repairMermaid('   \n  '), { source: '   \n  ', changed: false, fixes: [] });
  assert.equal(repairMermaid('%% only a comment').changed, false);
});

test('repair: a v11 shape block is copied verbatim', () => {
  const source = 'flowchart LR\n  A@{ shape: rect, label: "x (y)" } --> B[Plain]';
  assert.equal(repairMermaid(source).changed, false);
});

test('repair: an unterminated shape block does not hang', () => {
  const result = repairMermaid('flowchart LR\n  A@{ shape: rect --> B');
  assert.equal(typeof result.source, 'string');
});

test('quoteLabel: quotes only labels that need it', () => {
  assert.equal(quoteLabel('plain text'), 'plain text');
  assert.equal(quoteLabel('f(x)'), '"f(x)"');
  assert.equal(quoteLabel('"ok"'), '"ok"');
  assert.equal(quoteLabel('"say "hi""'), '"say #quot;hi#quot;"');
  assert.equal(quoteLabel('a | b'), '"a | b"');
});
