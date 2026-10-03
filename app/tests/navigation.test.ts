/**
 * Agent navigation and web-source tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/navigation.test.ts
 *
 * `navigated` events may carry a typed target (document, calendar,
 * conversation, audit, source). Events from older builds have no target and
 * still switch the view; malformed targets are dropped, never half-applied.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { AgentEvent, NavigatedEvent } from '../src/features/agent/events.ts';
import {
  navigationFromEvent,
  parseTarget,
  peekTarget,
  publishTarget,
  subscribeTargets,
  takeTarget,
} from '../src/features/agent/navigation.ts';
import {
  initialTranscript,
  passagesFromDetail,
  reduceAll,
  webSourcesFromDetail,
} from '../src/features/agent/reducer.ts';
import { documentHit } from '../src/features/ask/searchResults.ts';

const RUN = 'run-nav';

function navigated(target: unknown, view = 'calendar'): NavigatedEvent {
  return { type: 'navigated', runId: RUN, view, focus: null, target } as NavigatedEvent;
}

test('every target kind parses', () => {
  assert.deepEqual(parseTarget({ kind: 'document', path: 'C:/docs/a.pdf', page: 4, passage: 'notice' }), {
    kind: 'document', path: 'C:/docs/a.pdf', page: 4, passage: 'notice',
  });
  assert.deepEqual(parseTarget({ kind: 'calendar', date: '2026-10-30', taskId: 't1', eventId: null }), {
    kind: 'calendar', date: '2026-10-30', taskId: 't1', eventId: null,
  });
  assert.deepEqual(parseTarget({ kind: 'conversation', conversationId: 'c1' }), { kind: 'conversation', conversationId: 'c1' });
  assert.deepEqual(parseTarget({ kind: 'audit', types: ['tool_call', 7], tool: 'web_search', from: null, to: null, text: '' }), {
    kind: 'audit', types: ['tool_call'], tool: 'web_search', from: null, to: null, text: null,
  });
  assert.deepEqual(parseTarget({ kind: 'source', sourceId: 's1' }), { kind: 'source', sourceId: 's1' });
});

test('malformed or unknown targets are dropped', () => {
  assert.equal(parseTarget(null), null);
  assert.equal(parseTarget({ kind: 'terminal', command: 'rm -rf /' }), null);
  assert.equal(parseTarget({ kind: 'document', page: 2 }), null, 'no path');
  assert.equal(parseTarget({ kind: 'calendar', date: null, taskId: null, eventId: null }), null, 'nothing to show');
  assert.equal(parseTarget({ kind: 'conversation', conversationId: '  ' }), null);
  const badPage = parseTarget({ kind: 'document', path: 'a.pdf', page: -3, passage: null });
  assert.equal(badPage?.kind === 'document' ? badPage.page : 'x', null);
  const badDate = parseTarget({ kind: 'calendar', date: 'next friday', taskId: 't', eventId: null });
  assert.equal(badDate?.kind === 'calendar' ? badDate.date : 'x', null);
});

test('older navigated events without a target still switch views', () => {
  const legacy = { type: 'navigated', runId: RUN, view: 'library', focus: 'src-1' } as unknown as NavigatedEvent;
  assert.deepEqual(navigationFromEvent(legacy), { view: 'library', focus: 'src-1', target: null });
  const withTarget = navigationFromEvent(navigated({ kind: 'calendar', date: '2026-11-05', taskId: 'abc', eventId: null }));
  assert.equal(withTarget.target?.kind, 'calendar');
});

test('targets wait for their view and are delivered once', () => {
  publishTarget({ kind: 'source', sourceId: 's9' });
  assert.deepEqual(peekTarget('source'), { kind: 'source', sourceId: 's9' }, 'peek leaves it pending');
  assert.deepEqual(takeTarget('source'), { kind: 'source', sourceId: 's9' });
  assert.equal(takeTarget('source'), null);

  const seen: string[] = [];
  const unsubscribe = subscribeTargets(t => seen.push(t.kind));
  publishTarget({ kind: 'conversation', conversationId: 'c2' });
  unsubscribe();
  publishTarget({ kind: 'conversation', conversationId: 'c3' });
  assert.deepEqual(seen, ['conversation']);
  assert.deepEqual(takeTarget('conversation'), { kind: 'conversation', conversationId: 'c3' }, 'latest wins');
});

test('the transcript ignores navigation but keeps web sources as cited passages', () => {
  const events: AgentEvent[] = [
    { type: 'run_started', runId: RUN, sessionId: 's', model: 'm', atMs: 1, warning: null },
    navigated({ kind: 'document', path: 'C:/docs/a.pdf', page: 2, passage: null }, 'ask'),
    {
      type: 'step_started', runId: RUN, stepId: 'w1', parentStepId: null, tool: 'web_search',
      label: 'Searching the web for “rust 2024”', args: { query: 'rust 2024' }, tier: 'read', atMs: 2,
    },
    {
      type: 'step_finished', runId: RUN, stepId: 'w1', ok: true, summary: '2 web results · SearXNG', durationMs: 5,
      detail: {
        provider: 'SearXNG',
        webSources: [
          { n: 3, title: 'Rust 2024', url: 'https://blog.rust-lang.org/2025/02/20/Rust-1.85.0.html', snippet: 'Rust 1.85 ships the 2024 edition.' },
          { n: 4, title: 'Bad scheme', url: 'javascript:alert(1)', snippet: '' },
        ],
      },
    },
  ];
  const state = reduceAll(initialTranscript(RUN, 0), events);
  assert.equal(state.status, 'running');
  assert.equal(state.passages.length, 1, 'only http(s) sources become citations');
  assert.deepEqual(state.passages[0], {
    n: 3,
    file: 'Rust 2024',
    path: 'https://blog.rust-lang.org/2025/02/20/Rust-1.85.0.html',
    page: null,
    heading: null,
    score: 0,
    text: 'Rust 1.85 ships the 2024 edition.',
    web: true,
  });
  assert.equal(webSourcesFromDetail({ webSources: 'nope' }).length, 0);
  assert.equal(passagesFromDetail(null).length, 0);
});

test('documents opened by the agent become viewer targets', () => {
  const hit = documentHit('C:\\docs\\Lease.pdf', 4, 'notice period');
  assert.equal(hit.fileName, 'Lease.pdf');
  assert.deepEqual(hit.page, { start: 4, end: 4 });
  assert.equal(hit.text, 'notice period');
  assert.equal(hit.url, null);
  assert.equal(documentHit('https://example.org/a', null, null).url, 'https://example.org/a');
});
