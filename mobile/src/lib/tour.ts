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

/** Open each requested screen with `open`, one after another, in the order asked. */
export function followTour(open: (screen: string, theme: ThemeChoice) => Promise<void>): Promise<() => void> {
  let queue = Promise.resolve();
  return listen<{ screen: string; theme: ThemeChoice }>(TOUR_EVENT, ({ payload }) => {
    queue = queue
      .then(() => open(payload.screen, payload.theme))
      .catch(error => {
        // No ready line follows, so the tour fails on its wait and keeps this in the log.
        console.error('Shodh tour: the screen could not be opened', error);
      });
  });
}
