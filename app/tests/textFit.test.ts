/**
 * The launch test's measure of text that does not fit (features/launch/textFit.ts): the
 * rule itself, and that the tour asks every view for it and fails on a single hit. The
 * measuring runs in the built app (the Windows launch job); here are its rule and wiring.
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/textFit.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { MAX_REPORTED, clipped, fieldTextFits, shortenedWithTitle } from '../src/features/launch/textFit.ts';
import { appDir, source, withoutComments } from './jsxScan.ts';

const box = { scrollWidth: 100, clientWidth: 100, scrollHeight: 20, clientHeight: 20, overflowX: 'visible', overflowY: 'visible' };

test('text wider than its box does not fit, however the box treats it', () => {
  assert.equal(clipped(box), null);
  // An ellipsis (overflow hidden) and text spilling out of its box (overflow visible).
  assert.equal(clipped({ ...box, scrollWidth: 101, overflowX: 'hidden' }), 'width');
  assert.equal(clipped({ ...box, scrollWidth: 101, overflowX: 'clip' }), 'width');
  assert.equal(clipped({ ...box, scrollWidth: 101, overflowX: 'visible' }), 'width');
  // One pixel is a hit: there is no tolerance.
  assert.equal(clipped({ ...box, scrollWidth: 100, clientWidth: 99, overflowX: 'hidden' }), 'width');
  // A box that scrolls shows all of its text.
  assert.equal(clipped({ ...box, scrollWidth: 400, overflowX: 'auto' }), null);
  assert.equal(clipped({ ...box, scrollWidth: 400, overflowX: 'scroll' }), null);
});

test('text taller than a box that hides the rest does not fit', () => {
  assert.equal(clipped({ ...box, scrollHeight: 40, overflowY: 'hidden' }), 'height');
  assert.equal(clipped({ ...box, scrollHeight: 21, overflowY: 'clip' }), 'height');
  // A box that grows or scrolls shows it.
  assert.equal(clipped({ ...box, scrollHeight: 40, overflowY: 'visible' }), null);
  assert.equal(clipped({ ...box, scrollHeight: 40, overflowY: 'auto' }), null);
});

test("a field's placeholder or value fits the room the field gives it", () => {
  assert.ok(fieldTextFits(63.2, 64));
  assert.ok(fieldTextFits(64, 64));
  assert.ok(!fieldTextFits(64.1, 64));
});

test('the measure reads every drawn element, and a hiding box for all the text in it', () => {
  const fit = withoutComments(source('features/launch/textFit.ts'));
  assert.match(fit, /doc\.body\.querySelectorAll<HTMLElement>\('\*'\)/);
  // No element, class or view is passed over by name.
  assert.doesNotMatch(fit, /allow|ignore|skipList|except/i);
  assert.match(fit, /const cutsX = hides\(style\.overflowX\) \? inside : own;/);
  assert.match(fit, /const cutsY = hides\(style\.overflowY\) \? inside : '';/);
  assert.ok(MAX_REPORTED >= 50);
});

test('the views tour measures every view in every theme and fails on one hit', () => {
  const host = withoutComments(source('features/launch/LaunchProbeHost.tsx'));
  assert.match(host, /payload\.kind === 'measure'/);
  assert.match(host, /clippedText\(document\)/);
  assert.match(host, /invoke\('launch_probe_text_fit', \{/);

  const probe = readFileSync(join(appDir, 'src-tauri', 'src', 'launch_probe.rs'), 'utf8');
  // Asked for inside the loop over themes and views, after the view settled and before its picture.
  const tour = probe.slice(probe.indexOf('async fn views_tour'), probe.indexOf('/// The event main'));
  const at = (text: string) => {
    const i = tour.indexOf(text);
    assert.ok(i >= 0, text);
    return i;
  };
  assert.ok(at('for theme in TOUR_THEMES') < at('for view in TOUR_VIEWS'));
  assert.ok(at('"kind": "show"') < at('tokio::time::sleep(TOUR_SETTLE)'));
  assert.ok(at('tokio::time::sleep(TOUR_SETTLE)') < at('"kind": "measure"'));
  assert.ok(at('"kind": "measure"') < at('Launch probe: view {view} in {theme} shown'));
  // Every hit is logged, the count is of all of them, and any at all fails the probe.
  assert.match(tour, /tracing::error!\("Launch probe: clipped \{view\} \{theme\} \{line\}"\);/);
  assert.match(tour, /clipped_total = clipped_total\.saturating_add\(fit\.count\);/);
  assert.match(tour, /if clipped_total > 0 \{\s+return Err\(/);
  // The report is kept only while the probe runs, and only for the tour's own views.
  assert.match(probe, /if !STARTED\.load\(Ordering::SeqCst\) \{\s+return;/);
});

test('a name shortened with an ellipsis passes only with its whole text in a title', () => {
  const name = 'What does the paper say about recall at small index sizes?';
  // Passes: an ellipsis, and the whole name in the element's title or its row's.
  assert.ok(shortenedWithTitle('ellipsis', name, [name, null]));
  assert.ok(shortenedWithTitle('ellipsis', name, [null, name]));
  assert.ok(shortenedWithTitle('ellipsis', `  ${name}\n`, [name.replace(/ /g, '  '), null]), 'spaces read as on screen');
  // Fails: no title.
  assert.ok(!shortenedWithTitle('ellipsis', name, [null, null]));
  assert.ok(!shortenedWithTitle('ellipsis', name, []));
  // Fails: a title that says something else (a part of it, or another name).
  assert.ok(!shortenedWithTitle('ellipsis', name, ['What does the paper say', null]));
  assert.ok(!shortenedWithTitle('ellipsis', name, ['Open chat', null]));
  // Fails: cut without an ellipsis, even with the title.
  assert.ok(!shortenedWithTitle('clip', name, [name, null]));
  assert.ok(!shortenedWithTitle('', name, [name, null]));
  // Fails: nothing to read.
  assert.ok(!shortenedWithTitle('ellipsis', '   ', ['   ', null]));

  // The measure lets only that through, for text cut to its width, never for height.
  const fit = withoutComments(source('features/launch/textFit.ts'));
  assert.match(fit, /if \(why === 'width' && shortenedWithTitle\(\s*style\.textOverflow,\s*el\.textContent \?\? '',\s*\[el\.getAttribute\('title'\), el\.closest\(ROW_OR_CONTROL\)\?\.getAttribute\('title'\) \?\? null\],\s*\)\) continue;/);
  // The sidebar's chat titles carry their whole text.
  assert.match(source('components/shell/ConversationHistory.tsx'), /<span className="text-small truncate" title=\{conversation\.title\}>\{conversation\.title\}<\/span>/);
});

test('a citation pill is at least as wide as its 24px target, so its target never spills past its text', () => {
  // `target-24` widens a small control's hit area with a centred pseudo-element; on a box
  // under 24px wide it reaches past the box, and the measure reads that as text that does
  // not fit. A pill with text is as wide as its target and grows with its number.
  const renderer = source('features/ask/MessageContentRenderer.tsx');
  assert.match(renderer, /'target-24 inline-flex items-center justify-center min-w-6 h-\[18px\] px-1 /);
  // The pill a wrong number gets keeps the same size.
  assert.match(source('features/agent/GroundingFlags.tsx'), /inline-flex items-center justify-center min-w-6 h-\[18px\] px-1 ml-0\.5 rounded-\[5px\]/);
});
