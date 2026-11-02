/**
 * Hidden formatting characters in approval prompts are shown, never applied. Run with
 * Node 22.6+:
 *   node --experimental-strip-types --test app/tests/hiddenChars.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { hasHidden, revealHidden } from '../src/features/agent/hiddenChars.ts';

test('bidi overrides, isolates and zero-width characters are made visible', () => {
  assert.equal(revealHidden('report‮txt.exe'), 'report⟨U+202E⟩txt.exe');
  assert.equal(revealHidden('a​b⁦c⁩﻿'), 'a⟨U+200B⟩b⟨U+2066⟩c⟨U+2069⟩⟨U+FEFF⟩');
  assert.equal(revealHidden('Fix the login (1 file, +1 −1) “quoted” — ok'), 'Fix the login (1 file, +1 −1) “quoted” — ok');
  assert.ok(hasHidden('x‭y'));
  assert.ok(!hasHidden('plain text'));
  // Repeated calls agree (no state left in the pattern).
  assert.ok(hasHidden('‏'));
  assert.ok(hasHidden('‏'));
});
