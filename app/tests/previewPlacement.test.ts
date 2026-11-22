/**
 * When the embedded preview and the floating chat may show (lib/previewPlacement). Run with
 * Node 22.6+:
 *   node --experimental-strip-types --test app/tests/previewPlacement.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { OVERLAY_SELECTORS, coveredBy, layerVisible } from '../src/lib/previewPlacement.ts';
import type { LayerState } from '../src/lib/previewPlacement.ts';

const window = { width: 1400, height: 900 };
const pane = { x: 300, y: 120, width: 1100, height: 780 };
const dockCorner = { x: window.width - 16 - 380, y: window.height - 16 - 470, width: 380, height: 470 };

test('a menu over the pane covers it', () => {
  const menu = { x: 900, y: 140, width: 220, height: 300 };
  assert.equal(coveredBy(pane, [menu], false, dockCorner), true);
  // Touching only the pane's edge is not over it.
  assert.equal(coveredBy(pane, [{ x: 80, y: 140, width: 220, height: 300 }], false), false);
});

test('a toast away from the pane and the dock corner does not', () => {
  const toast = { x: 16, y: 820, width: 260, height: 60 };
  assert.equal(coveredBy(pane, [toast], false, dockCorner), false);
  // The same toast in the chat's corner does (the chat's webview would hide it).
  const cornerToast = { x: 1030, y: 820, width: 356, height: 60 };
  assert.equal(coveredBy({ x: 0, y: 0, width: 200, height: 200 }, [cornerToast], false, dockCorner), true);
  assert.equal(coveredBy(pane, [], false, dockCorner), false);
});

test('any modal dialog covers it wherever it is', () => {
  assert.equal(coveredBy(pane, [], true), true);
  assert.equal(coveredBy({ x: 0, y: 0, width: 0, height: 0 }, [], true, null), true);
});

test('hidden unless on Artifacts, pane shown, not popped out, not covered and the window visible', () => {
  const up: LayerState = { onArtifacts: true, paneShown: true, poppedOut: false, covered: false, windowVisible: true };
  assert.equal(layerVisible(up), true);
  const flips: Array<Partial<LayerState>> = [
    { onArtifacts: false },
    { paneShown: false },
    { poppedOut: true },
    { covered: true },
    { windowVisible: false },
  ];
  for (const flip of flips) {
    assert.equal(layerVisible({ ...up, ...flip }), false, JSON.stringify(flip));
  }
});

test('the overlays the preview must never hide are all watched, the in-page chat is not', () => {
  for (const selector of ['[role="dialog"][aria-modal="true"]', '[role="menu"]', '[role="listbox"]', '[data-radix-popper-content-wrapper]', '[data-sonner-toast]', '[cmdk-root]', '[data-shodh-overlay]']) {
    assert.ok(OVERLAY_SELECTORS.includes(selector), selector);
  }
  // The in-page chat's panel is a non-modal dialog: never counted as covering.
  assert.ok(!OVERLAY_SELECTORS.includes('[role="dialog"]'));
});
