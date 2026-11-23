/**
 * When the embedded preview and the floating chat may show (lib/previewPlacement). Run with
 * Node 22.6+:
 *   node --experimental-strip-types --test app/tests/previewPlacement.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  OVERLAY_SELECTORS,
  coveredBy,
  coveringOverlay,
  dockCornerBox,
  hiddenWhileText,
  layerVisible,
  overlayName,
} from '../src/lib/previewPlacement.ts';
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

const PANEL = { width: 380, height: 470 };

test('the chat corner sits left of a pane docked at the right', () => {
  assert.deepEqual(dockCornerBox(window, PANEL, 16), dockCorner);
  assert.deepEqual(dockCornerBox(window, PANEL, 16, 560), { x: 1400 - 560 - 16 - 380, y: 900 - 16 - 470, width: 380, height: 470 });
  // A window too small for the panel: the corner shrinks to fit, never off the window.
  const small = dockCornerBox({ width: 300, height: 200 }, PANEL, 16, 0);
  assert.deepEqual(small, { x: 16, y: 16, width: 268, height: 168 });
  // A nonsense inset counts as none.
  assert.deepEqual(dockCornerBox(window, PANEL, 16, Number.NaN), dockCorner);
  assert.deepEqual(dockCornerBox(window, PANEL, 16, -40), dockCorner);
});

test('after a Creator finish with a design check, the page is shown, not covered', () => {
  // The run seen on PR #198: the agent's design_check opened its pane over the right half of
  // Artifacts (x 720–1400, full height), so the page stayed hidden after every Creator build.
  const overPane = { name: 'Design check', box: { x: 720, y: 0, width: 680, height: 900 } };
  assert.equal(coveringOverlay(pane, [overPane], null, dockCorner), 'Design check');
  // Docked beside the page, the pane takes its own column: the page's box ends where the pane
  // starts and the chat's corner moves left of it, so nothing of the app's is above the page.
  const docked = { name: 'Design check', box: { x: 840, y: 0, width: 560, height: 900 } };
  const narrowed = { x: 300, y: 120, width: 540, height: 780 };
  assert.equal(coveringOverlay(narrowed, [docked], null, dockCornerBox(window, PANEL, 16, 560)), null);
  assert.equal(coveredBy(narrowed, [docked.box], false, dockCornerBox(window, PANEL, 16, 560)), false);
  // Without moving the chat's corner the docked pane would still hide the page.
  assert.equal(coveringOverlay(narrowed, [docked], null, dockCornerBox(window, PANEL, 16, 0)), 'Design check');
});

test('what covers the page is named, so the pane can say so', () => {
  const menu = { name: 'a menu', box: { x: 900, y: 140, width: 220, height: 300 } };
  const toast = { name: 'a notification', box: { x: 16, y: 820, width: 260, height: 60 } };
  // The first overlay that is over the page or the chat's corner names it.
  assert.equal(coveringOverlay(pane, [toast, menu], null, dockCorner), 'a menu');
  assert.equal(coveringOverlay(pane, [toast], null, dockCorner), null);
  // A modal dialog anywhere covers it, by its own name.
  assert.equal(coveringOverlay(pane, [], 'Settings', dockCorner), 'Settings');
  // coveredBy and coveringOverlay always agree.
  for (const overlays of [[], [toast], [menu], [toast, menu]]) {
    for (const modal of [null, 'a dialog']) {
      assert.equal(
        coveredBy(pane, overlays.map(o => o.box), modal !== null, dockCorner),
        coveringOverlay(pane, overlays, modal, dockCorner) !== null,
        JSON.stringify({ overlays, modal }),
      );
    }
  }
  assert.equal(hiddenWhileText('Design check'), 'Hidden while Design check is open');
});

test('an overlay is named by its own label, else by what it is', () => {
  assert.equal(overlayName({ label: 'Design check', role: null, toast: false, palette: false }), 'Design check');
  assert.equal(overlayName({ label: '  ', role: 'menu', toast: false, palette: false }), 'a menu');
  assert.equal(overlayName({ label: null, role: 'listbox', toast: false, palette: false }), 'a list');
  assert.equal(overlayName({ label: null, role: 'dialog', toast: false, palette: false }), 'a dialog');
  assert.equal(overlayName({ label: null, role: 'alertdialog', toast: false, palette: false }), 'a dialog');
  assert.equal(overlayName({ label: null, role: null, toast: true, palette: false }), 'a notification');
  assert.equal(overlayName({ label: null, role: null, toast: false, palette: true }), 'the command palette');
  assert.equal(overlayName({ label: null, role: null, toast: false, palette: false }), 'a panel');
  // A label is cut short: the status line stays one line.
  assert.ok(overlayName({ label: 'x'.repeat(200), role: null, toast: false, palette: false }).length <= 40);
});
