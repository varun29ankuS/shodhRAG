/**
 * Hidden formatting characters in approval prompts are shown, never applied. Run with
 * Node 22.6+:
 *   node --experimental-strip-types --test app/tests/hiddenChars.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { HIDDEN_RANGES, hasHidden, revealHidden } from '../src/features/agent/hiddenChars.ts';

const here = dirname(fileURLToPath(import.meta.url));

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

test('tag characters, variation selectors and controls are revealed too', () => {
  assert.equal(revealHidden('Fix \u{E0041}\u{E0042}'), 'Fix ⟨U+E0041⟩⟨U+E0042⟩');
  assert.equal(revealHidden('a\u00ADb\uFE0Fc'), 'a⟨U+00AD⟩b⟨U+FE0F⟩c');
  assert.equal(revealHidden('line\r\nnext\ttab'), 'line⟨U+000D⟩\nnext\ttab', 'line breaks and tabs stay');
  assert.ok(hasHidden('\u{E007F}'));
});

test('the hidden set is exactly the one Shodh refuses in a command', () => {
  // The Rust list: `hidden_character` in harness/run_command/mod.rs.
  const source = readFileSync(join(here, '../../crates/shodh-rag/src/harness/run_command/mod.rs'), 'utf8');
  const body = /pub fn hidden_character\(c: char\) -> bool \{([\s\S]*?)\n\}/.exec(source);
  assert.ok(body, 'hidden_character is in run_command/mod.rs');
  assert.match(body[1], /c\.is_control\(\) && c != '\\n' && c != '\\t'/, 'controls but line breaks and tabs');
  const rust: [number, number][] = [
    // `char::is_control` is Unicode's Cc: U+0000–U+001F and U+007F–U+009F, less \t and \n.
    [0x00, 0x08],
    [0x0b, 0x1f],
    [0x7f, 0x9f],
  ];
  for (const m of body[1].matchAll(/'\\u\{([0-9A-Fa-f]+)\}'(?:\.\.='\\u\{([0-9A-Fa-f]+)\}')?/g)) {
    const from = parseInt(m[1], 16);
    rust.push([from, m[2] ? parseInt(m[2], 16) : from]);
  }
  const expand = (ranges: readonly (readonly [number, number])[]) => {
    const set = new Set<number>();
    for (const [from, to] of ranges) for (let cp = from; cp <= to; cp++) set.add(cp);
    return [...set].sort((a, b) => a - b);
  };
  assert.deepEqual(expand(HIDDEN_RANGES), expand(rust));
});
