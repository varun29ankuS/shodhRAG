/**
 * CI's screenshot tour, in a debug build only: main.tsx imports this behind
 * `__SHODH_DEMO__` (false in a release build, so its JavaScript does not
 * contain it), and only a debug build's Rust side sends its requests
 * (mobile/src-tauri/src/tour.rs). Each request opens a screen again from the
 * start, in the theme asked for, in the same running app: the tour never
 * relaunches the app, which killed the emulator. The screen then writes its
 * own ready line, as at launch.
 */
import { listen } from '@tauri-apps/api/event';
import type { ThemeChoice } from './theme';

const TOUR_EVENT = 'shodh://tour';

/** The request for the app's own start, as a tester first sees it (the pair screen on a new phone). */
const OWN_START = 'start';

/** The stylesheet of CI's measurement without blur (tour.rs, `debug.shodh.noblur`). */
const NO_BLUR_STYLE = 'shodh-tour-noblur';

/**
 * Every `backdrop-filter` off, for CI's measurement of what the frosted glass
 * costs the emulator's software renderer. The translucent fill under it
 * (`--glass`) stays, so the blur is the one thing that differs.
 */
function drawWithoutBlur() {
  if (document.getElementById(NO_BLUR_STYLE)) return;
  const style = document.createElement('style');
  style.id = NO_BLUR_STYLE;
  style.textContent = '*, *::before, *::after { backdrop-filter: none !important; -webkit-backdrop-filter: none !important; }';
  document.head.append(style);
}

/**
 * Open each requested screen with `open`, one after another, in the order
 * asked: a screen's name, or null for the app's own start.
 */
export function followTour(open: (screen: string | null, theme: ThemeChoice) => Promise<void>): Promise<() => void> {
  let queue = Promise.resolve();
  return listen<{ screen: string; theme: ThemeChoice; noBlur: boolean }>(TOUR_EVENT, ({ payload }) => {
    if (payload.noBlur) drawWithoutBlur();
    queue = queue
      .then(() => open(payload.screen === OWN_START ? null : payload.screen, payload.theme))
      .catch(error => {
        // No ready line follows, so the tour fails on its wait and keeps this in the log.
        console.error('Shodh tour: the screen could not be opened', error);
      });
  });
}
