/**
 * Focus returned after a long run only where the reader has not moved it.
 *   node --experimental-strip-types --test app/tests/focusReturn.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { focusIsFree } from '../src/lib/focusReturn.ts';

interface Node {
  name: string;
}

const body: Node = { name: 'body' };
const insideFigure: Node = { name: 'status line in the diagram' };
const composer: Node = { name: 'composer' };
const figure = { contains: (n: Node | null) => n === insideFigure };

test('focus may move when nothing has it, or it is still in the diagram', () => {
  assert.equal(focusIsFree<Node>(null, body, figure), true);
  assert.equal(focusIsFree(body, body, figure), true, 'the focused card was removed: focus fell to the body');
  assert.equal(focusIsFree(insideFigure, body, figure), true);
});

test('focus the reader moved elsewhere is never taken', () => {
  assert.equal(focusIsFree(composer, body, figure), false, 'typing in the composer during a redraw');
  assert.equal(focusIsFree(composer, body, null), false);
});
