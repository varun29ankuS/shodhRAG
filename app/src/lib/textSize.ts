/**
 * Settings → Appearance → Text size: how large the whole interface is drawn
 * (the windows' zoom, applied by the backend on top of the operating system's
 * display scaling; `TextSize` in app_settings.rs has the same table). Ctrl + /
 * Ctrl − step through the sizes and Ctrl 0 goes back to Default.
 *
 * Pure (its one import has its extension) so it is unit-tested directly with Node
 * (`app/tests/textSize.test.ts`).
 */

import { IS_MAC_PLATFORM, matchesShortcut } from './shortcuts.ts';
import type { KeyChord } from './shortcuts.ts';

export const TEXT_SIZES = ['small', 'default', 'large', 'larger'] as const;

export type TextSize = typeof TEXT_SIZES[number];

export const DEFAULT_TEXT_SIZE: TextSize = 'default';

export const TEXT_SIZE_LABELS: Record<TextSize, string> = {
  small: 'Small',
  default: 'Default',
  large: 'Large',
  larger: 'Larger',
};

/** The windows' zoom for each size. */
export const TEXT_SIZE_ZOOM: Record<TextSize, number> = {
  small: 0.9,
  default: 1,
  large: 1.15,
  larger: 1.3,
};

export function isTextSize(value: unknown): value is TextSize {
  return typeof value === 'string' && (TEXT_SIZES as readonly string[]).includes(value);
}

/** A saved size; anything else (settings from before the setting existed) is Default. */
export function parseTextSize(value: unknown): TextSize {
  return isTextSize(value) ? value : DEFAULT_TEXT_SIZE;
}

/**
 * The size after a shortcut: one step larger or smaller, stopping at Small and Larger
 * (a press past either end changes nothing), or back to Default.
 */
export function stepTextSize(current: TextSize, step: 'larger' | 'smaller' | 'reset'): TextSize {
  if (step === 'reset') return DEFAULT_TEXT_SIZE;
  const i = TEXT_SIZES.indexOf(current);
  const next = step === 'larger' ? Math.min(TEXT_SIZES.length - 1, i + 1) : Math.max(0, i - 1);
  return TEXT_SIZES[next];
}

/** The parts of a key event the text-size shortcuts read. */
export interface TextSizeKey extends KeyChord {
  /** Something already handled it (the PDF viewer and the pop-out zoom their own content). */
  defaultPrevented: boolean;
  /** Held down: auto-repeat would race through the sizes. */
  repeat: boolean;
}

/**
 * The step a key press asks for, or null when it is not a text-size shortcut, or when
 * it is but belongs to something else: a press already handled (Ctrl + in the PDF
 * viewer zooms the page, not the app) or an auto-repeat.
 */
export function textSizeStepFor(e: TextSizeKey, mac: boolean = IS_MAC_PLATFORM): 'larger' | 'smaller' | 'reset' | null {
  if (e.defaultPrevented || e.repeat) return null;
  if (matchesShortcut(e, 'textLarger', mac)) return 'larger';
  if (matchesShortcut(e, 'textSmaller', mac)) return 'smaller';
  if (matchesShortcut(e, 'textReset', mac)) return 'reset';
  return null;
}

/**
 * Orders text-size saves: only the answer to the latest request may set the size, so
 * an older answer arriving after a newer step never puts the size back.
 */
export class LatestRequest {
  private latest = 0;

  /** A new request; its number. */
  next(): number {
    this.latest += 1;
    return this.latest;
  }

  /** Whether the answer to request `id` is still the one that counts. */
  isLatest(id: number): boolean {
    return id === this.latest;
  }
}
