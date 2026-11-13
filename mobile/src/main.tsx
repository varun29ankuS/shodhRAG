import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import '@fontsource-variable/geist';
import '@fontsource-variable/newsreader/opsz.css';
import { App, type Start } from './App';
import { tauriLink, type LinkSource } from './lib/link';
import { startNav } from './lib/start';
import './theme.css';

// The desktop's colour tokens (tokens.ts), before anything renders.
const tokens = document.createElement('style');
tokens.textContent = __SHODH_TOKENS__;
document.head.prepend(tokens);

/**
 * Where to open: always the app's own start, except in a debug build asked
 * for a screen (CI's screenshots). `demo-` screens show a scripted computer;
 * `__SHODH_DEMO__` is false in a release build, so that code is not in it.
 */
async function opening(): Promise<{ source: LinkSource; start: Start | null }> {
  const screen = await tauriLink.startScreen().catch(() => null);
  if (!screen) return { source: tauriLink, start: null };
  if (__SHODH_DEMO__ && screen.startsWith('demo-')) {
    const { demoLink, demoNav } = await import('./lib/demo');
    // The ready line is still the app's own, so CI waits for it as for any screen.
    return { source: { ...demoLink(), ready: tauriLink.ready }, start: { screen, nav: demoNav(screen.slice('demo-'.length)) } };
  }
  return { source: tauriLink, start: { screen, nav: startNav(screen) } };
}

const root = document.getElementById('root');
if (!root) throw new Error('index.html has no #root');
const mount = root;
void opening().then(({ source, start }) =>
  createRoot(mount).render(
    <StrictMode>
      <App source={source} start={start} />
    </StrictMode>,
  ),
);
