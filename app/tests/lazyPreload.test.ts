/**
 * A view whose code was preloaded renders on its first render instead of
 * suspending (which would show its loading placeholder for a frame): React.lazy
 * reads the module synchronously from `settledThenable`. Rendered with
 * react-dom/server, which resolves lazy components the same way the client
 * does. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/lazyPreload.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
// Default imports: both packages are CommonJS.
import React from 'react';
import ReactDOMServer from 'react-dom/server';
import { settledThenable } from '../src/lib/viewPreload.ts';

const { Suspense, createElement, lazy } = React;
const { renderToString } = ReactDOMServer;

function View() {
  return createElement('p', null, 'library content');
}

function render(component: ReturnType<typeof lazy>): string {
  return renderToString(createElement(Suspense, { fallback: createElement('p', null, 'loading placeholder') }, createElement(component)));
}

test('a preloaded view renders on the first render, without its placeholder', () => {
  const Preloaded = lazy(() => settledThenable({ default: View }));
  const html = render(Preloaded);
  assert.match(html, /library content/);
  assert.doesNotMatch(html, /loading placeholder/);
});

test('a view loaded through a promise suspends on its first render (what preloading avoids)', () => {
  const Loading = lazy(() => Promise.resolve({ default: View }));
  const html = render(Loading);
  assert.match(html, /loading placeholder/);
  assert.doesNotMatch(html, /library content/);
});
