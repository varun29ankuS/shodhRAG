/**
 * The cue shown when the assistant changes the view. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/viewCue.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { VIEW_CUE_MS, cueAnnouncement, initialCueTracker, nextCue } from '../src/lib/viewCue.ts';
import type { CueInput, CueTracker } from '../src/lib/viewCue.ts';

type V = 'ask' | 'library' | 'tasks';

/** Feeds renders through the tracker and collects the cues. */
function run(start: CueInput<V>, renders: CueInput<V>[]): (V | null)[] {
  let tracker: CueTracker<V> = initialCueTracker(start);
  return renders.map(input => {
    const result = nextCue(tracker, input);
    tracker = result.tracker;
    return result.cue;
  });
}

test('the assistant opening another view cues it once', () => {
  assert.deepEqual(
    run({ seq: 0, view: null, tab: 'ask' }, [
      { seq: 1, view: 'library', tab: 'library' },
      { seq: 1, view: 'library', tab: 'library' },
    ]),
    ['library', null],
  );
});

test("the user's own switches never cue", () => {
  assert.deepEqual(
    run({ seq: 0, view: null, tab: 'ask' }, [
      { seq: 0, view: null, tab: 'library' },
      { seq: 0, view: null, tab: 'tasks' },
    ]),
    [null, null],
  );
  // After an assistant navigation, later clicks still show nothing.
  assert.deepEqual(
    run({ seq: 3, view: 'library', tab: 'library' }, [
      { seq: 3, view: 'library', tab: 'ask' },
      { seq: 3, view: 'library', tab: 'library' },
    ]),
    [null, null],
  );
});

test('a navigation to the view already open does not cue', () => {
  assert.deepEqual(run({ seq: 0, view: null, tab: 'library' }, [{ seq: 1, view: 'library', tab: 'library' }]), [null]);
});

test('each new navigation cues again, and a navigation seen at mount is not replayed', () => {
  assert.deepEqual(
    run({ seq: 4, view: 'tasks', tab: 'tasks' }, [
      { seq: 5, view: 'library', tab: 'library' },
      { seq: 6, view: 'ask', tab: 'ask' },
    ]),
    ['library', 'ask'],
  );
});

test('the cue is announced in full and fades on the same clock it is removed on', () => {
  assert.equal(cueAnnouncement('Library'), 'Shodh opened Library');
  const css = readFileSync(new URL('../src/index.css', import.meta.url), 'utf8');
  const rule = /\.shell-cue\s*\{\s*animation:\s*shell-cue\s+(\d+)ms/.exec(css);
  assert.ok(rule, '.shell-cue animates with shell-cue');
  assert.equal(Number(rule[1]), VIEW_CUE_MS);
  const reduced = /@media \(prefers-reduced-motion: reduce\)\s*\{\s*\.shell-view-enter,([^{]*)\{\s*animation: none;/.exec(css);
  assert.ok(reduced, 'shell motion has a reduced-motion block');
  assert.match(reduced[1], /\.shell-cue,/);
});

test('the cue never takes focus and stays mounted as a polite live region', () => {
  const cue = readFileSync(new URL('../src/components/shell/AssistantViewCue.tsx', import.meta.url), 'utf8');
  assert.doesNotMatch(cue, /\.focus\(/);
  assert.match(cue, /<span role="status" aria-live="polite" aria-atomic="true"[^>]*>\s*\{cue && \(/);
  const app = readFileSync(new URL('../src/App-SplitView.tsx', import.meta.url), 'utf8');
  assert.match(app, /useAssistantViewCue\(navigation\?\.seq \?\? 0, navigation\?\.view \?\? null, activeTab\)/);
  assert.match(app, /<AssistantViewCue cue=\{viewCue\?\.view === viewTab \? viewCue : null\} \/>/);
});
