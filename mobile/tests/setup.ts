/**
 * The phone's WebView, as far as the screens' tests need it: the phone's
 * light or dark setting (`matchMedia`, which tests switch with
 * `setPhoneTheme`), and the few browser calls jsdom does not have.
 */
import { cleanup } from '@testing-library/react';
import { MotionGlobalConfig } from 'framer-motion';
import { afterEach, beforeEach } from 'vitest';
import { resetPhoneTheme } from './phone';

// Springs settle at once: the tests read what each screen shows, not how it moves
// (tests/rules.test.ts checks what moves).
MotionGlobalConfig.skipAnimations = true;

beforeEach(() => {
  resetPhoneTheme();
});

afterEach(() => {
  cleanup();
  try {
    window.localStorage.clear();
  } catch {
    // No storage: nothing kept.
  }
  delete document.documentElement.dataset.theme;
});

if (typeof Element !== 'undefined' && !Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = () => undefined;
}
