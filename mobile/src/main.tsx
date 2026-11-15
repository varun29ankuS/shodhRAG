import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import '@fontsource-variable/inter';
import { App, type Start } from './App';
import { tauriLink, type LinkSource } from './lib/link';
import { startNav } from './lib/start';
import type { ThemeChoice } from './lib/theme';
import './theme.css';

// The desktop's colour tokens (tokens.ts), before anything renders.
const tokens = document.createElement('style');
tokens.textContent = __SHODH_TOKENS__;
document.head.prepend(tokens);

interface Opening {
  source: LinkSource;
  start: Start | null;
}

/**
 * Where to open: always the app's own start, except in a debug build asked
 * for a screen (CI's screenshots). `demo-` screens show a scripted computer;
 * `__SHODH_DEMO__` is false in a release build, so that code is not in it.
 */
async function opening(screen: string | null, theme?: ThemeChoice): Promise<Opening> {
  if (!screen) return { source: tauriLink, start: null };
  if (__SHODH_DEMO__ && screen.startsWith('demo-')) {
    const { demoLink, demoNav } = await import('./lib/demo');
    // The ready line is still the app's own, so CI waits for it as for any screen.
    return { source: { ...demoLink(), ready: tauriLink.ready }, start: { screen, theme, nav: demoNav(screen.slice('demo-'.length)) } };
  }
  return { source: tauriLink, start: { screen, theme, nav: startNav(screen) } };
}

const element = document.getElementById('root');
if (!element) throw new Error('index.html has no #root');
const root = createRoot(element);

// Each opening is a new App (its key), so a screen the tour asks for starts
// as fresh as at launch: the splash, the link's state and the ready line.
let openings = 0;
function show({ source, start }: Opening) {
  openings += 1;
  root.render(
    <StrictMode>
      <App key={openings} source={source} start={start} />
    </StrictMode>,
  );
}

// CI's screenshot tour (lib/tour.ts): every screen in one run of the app.
if (__SHODH_DEMO__) {
  void import('./lib/tour').then(({ followTour }) => followTour((screen, theme) => opening(screen, theme).then(show)));
}

void tauriLink
  .startScreen()
  .catch(() => null)
  .then(screen => opening(screen))
  .then(show);
