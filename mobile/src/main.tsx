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
async function opening(screen: string | null): Promise<Opening> {
  if (!screen) return { source: tauriLink, start: null };
  if (__SHODH_DEMO__ && screen.startsWith('demo-')) {
    const { demoLink, demoNav } = await import('./lib/demo');
    // The ready line is still the app's own, so CI waits for it as for any screen.
    return { source: { ...demoLink(), ready: tauriLink.ready }, start: { screen, nav: demoNav(screen.slice('demo-'.length)) } };
  }
  return { source: tauriLink, start: { screen, nav: startNav(screen) } };
}

const element = document.getElementById('root');
if (!element) throw new Error('index.html has no #root');
const root = createRoot(element);

// Each opening is a new App (its key), so a screen the tour asks for starts
// as fresh as at launch: the splash, the link's state and the ready line.
let openings = 0;
function show({ source, start }: Opening, theme?: ThemeChoice) {
  openings += 1;
  root.render(
    <StrictMode>
      <App key={openings} source={source} start={start} theme={theme} />
    </StrictMode>,
  );
}

// CI's screenshot tour (lib/tour.ts): every screen in one run of the app.
if (__SHODH_DEMO__) {
  void import('./lib/tour').then(({ followTour }) => followTour((screen, theme) => opening(screen).then(next => show(next, theme))));
}

void tauriLink
  .startScreen()
  .catch(() => null)
  .then(opening)
  .then(next => show(next));
