/**
 * The app draws edge to edge, under Android's status bar and gesture bar.
 * The WebView reports neither to the page (env(safe-area-inset-*) is 0 in
 * it), so the phone's own measure reaches the page as `--system-inset-*`, and
 * every layer that reaches an edge of the window keeps that edge's inset
 * clear. (That every control is inside one of those layers is checked on
 * every screen, in tests/screens.test.tsx.)
 */
import { act, render, renderHook, screen, waitFor } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from '../src/App';
import { applyInsets, SIDES, useSystemInsets } from '../src/lib/insets';
import { memoryDevice, MemoryInsets } from './device';
import { LAYERS, SIMULATED, reserved, stylesheet, type Edge } from './insets';
import { fakeSource, silentComputer } from './phone';

const NONE: Record<Edge, number> = { top: 0, right: 0, bottom: 0, left: 0 };

const root = () => document.documentElement.style;

describe('the system bars in the stylesheet', () => {
  it('are read through --inset-*: the larger of env() and the phone\'s own measure', () => {
    for (const side of SIDES) {
      const definition = new RegExp(`--inset-${side}:\\s*([^;]+);`).exec(stylesheet)?.[1].trim();
      expect(definition).toBe(`max(env(safe-area-inset-${side}, 0px), var(--system-inset-${side}, 0px))`);
    }
    // Nothing else reads env(): it is 0 in the phone's WebView, and an edge
    // written with it alone ends up under the bar.
    for (const side of ['top'] as const) {
      const uses = stylesheet.split(`env(safe-area-inset-${side}`).length - 1;
      expect(uses, `env(safe-area-inset-${side}) outside the --inset-${side} definition`).toBe(1);
    }
  });

  it('keep every edge of every layer clear of the bars', () => {
    for (const [layer, edges] of Object.entries(LAYERS)) {
      for (const [edge, reserve] of Object.entries(edges) as Array<[Edge, NonNullable<(typeof edges)[Edge]>]>) {
        const where = `.${layer}'s ${edge} (${reserve.rule} ${reserve.property})`;
        const withBars = reserved(reserve.rule, reserve.property, SIMULATED);
        const without = reserved(reserve.rule, reserve.property, NONE);
        // The inset is added to the layer's own room, never taken as part of it.
        expect(withBars - without, `${where} grows with the ${edge} inset`).toBe(SIMULATED[edge]);
        expect(withBars, `${where} clears the ${edge} inset`).toBeGreaterThanOrEqual(SIMULATED[edge] + reserve.beyond);
      }
    }
  });

  it('put the bars 16px under the status bar, and the large titles after them', () => {
    for (const rule of ['.topbar', '.navbar', '.reader-bar']) expect(reserved(rule, 'padding-top', NONE), rule).toBe(16);
  });
});

describe("the phone's measure", () => {
  it('is written to <html> at the start and on each change, and no longer once the app is gone', async () => {
    const insets = new MemoryInsets({ top: 24, right: 0, bottom: 24.4, left: 0 });
    const { unmount } = renderHook(() => useSystemInsets(insets));
    await waitFor(() => expect(root().getPropertyValue('--system-inset-top')).toBe('24px'));
    expect(root().getPropertyValue('--system-inset-bottom')).toBe('24.4px');
    expect(root().getPropertyValue('--system-inset-left')).toBe('0px');
    act(() => insets.change({ top: 0, right: 48, bottom: 0, left: 0 }));
    expect(root().getPropertyValue('--system-inset-top')).toBe('0px');
    expect(root().getPropertyValue('--system-inset-right')).toBe('48px');
    unmount();
    insets.change({ top: 30, right: 0, bottom: 0, left: 0 });
    expect(root().getPropertyValue('--system-inset-top')).toBe('0px');
  });

  it('is ignored when it is not four sizes', () => {
    applyInsets({ top: 24, right: 0, bottom: 24, left: 0 });
    for (const bad of [null, { top: 1 }, { top: -1, right: 0, bottom: 0, left: 0 }, { top: Number.NaN, right: 0, bottom: 0, left: 0 }, { top: '24', right: 0, bottom: 0, left: 0 }]) {
      applyInsets(bad);
      expect(root().getPropertyValue('--system-inset-top')).toBe('24px');
    }
  });

  it('reaches the page from the phone the app runs on', async () => {
    const device = memoryDevice({ insets: { top: 28, right: 0, bottom: 20, left: 0 } });
    render(<App source={fakeSource(silentComputer(), { state: 'unpaired' })} device={device} />);
    await screen.findByRole('heading', { name: 'Pair with your computer' });
    await waitFor(() => expect(root().getPropertyValue('--system-inset-top')).toBe('28px'));
    expect(root().getPropertyValue('--system-inset-bottom')).toBe('20px');
  });
});
