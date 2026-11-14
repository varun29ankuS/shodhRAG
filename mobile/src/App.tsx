/**
 * The app: the splash while the link starts, pairing when this phone has no
 * computer yet, and otherwise the sections (lib/nav.ts) with the floating
 * tab bar, the menu, the Reader and the approval sheet over them.
 */
import { useCallback, useEffect, useRef, useState } from 'react';
import { AnimatePresence, MotionConfig } from 'framer-motion';
import { Drawer } from './components/Drawer';
import { NoteView } from './components/NoteView';
import { Splash } from './components/Splash';
import { TabBar } from './components/TabBar';
import { LinkProvider, NavProvider, NoteProvider, ThemeProvider, useLink, useNav, useThemeChoice } from './lib/context';
import { DataProvider, useData } from './lib/data';
import type { LinkSource } from './lib/link';
import { askScreen, type Nav } from './lib/nav';
import { Approvals, ApprovalSheet } from './screens/Approvals';
import { Ask } from './screens/Ask';
import { ComingSoon } from './screens/ComingSoon';
import { Conversations } from './screens/Conversations';
import { Failed } from './screens/Failed';
import { Home } from './screens/Home';
import { Library } from './screens/Library';
import { Pair } from './screens/Pair';
import { Reader } from './screens/Reader';
import { Settings } from './screens/Settings';
import { Workspaces } from './screens/Workspaces';

/**
 * Where a debug build was asked to open (CI's screenshots, lib/start.ts): the
 * screen's name for the ready line, and where the app is.
 */
export interface Start {
  screen: string;
  nav: Nav | null;
}

export function App({ source, start = null }: { source: LinkSource; start?: Start | null }) {
  const [pairing, setPairing] = useState(start?.screen === 'pair');
  const openPairing = useCallback(() => setPairing(true), []);
  return (
    <MotionConfig reducedMotion="user">
      <ThemeProvider>
        <LinkProvider source={source} openPairing={openPairing}>
          <NavProvider initial={start?.nav ?? undefined}>
            <NoteProvider>
              <DataProvider>
                <Shell start={start} pairing={pairing} closePairing={() => setPairing(false)} />
              </DataProvider>
            </NoteProvider>
          </NavProvider>
        </LinkProvider>
      </ThemeProvider>
    </MotionConfig>
  );
}

function Shell({ start, pairing, closePairing }: { start: Start | null; pairing: boolean; closePairing: () => void }) {
  const { status, source } = useLink();
  const { theme } = useThemeChoice();
  const unpaired = status.state === 'unpaired' || status.state === 'refused';
  const showPair = pairing || (unpaired && start === null);

  const screen =
    status.state === 'starting' ? null : status.state === 'failed' ? 'failed' : showPair ? 'pair' : (start?.screen ?? 'home');
  // The first screen shown, once: the line CI's launch test waits for.
  const told = useRef(false);
  useEffect(() => {
    if (screen === null || told.current) return;
    told.current = true;
    source.ready(screen, theme).catch(() => {
      // The app logged why it is not ready, and the screen says it too.
    });
  }, [screen, theme, source]);

  return (
    <div className="app">
      <span className="glow" aria-hidden="true" />
      {status.state === 'failed' ? (
        <Failed message={status.message} />
      ) : showPair ? (
        <Pair refusal={status.state === 'refused' ? status.message : null} onClose={unpaired && !pairing ? null : closePairing} />
      ) : (
        <Main />
      )}
      <AnimatePresence>{status.state === 'starting' && <Splash key="splash" />}</AnimatePresence>
      <NoteView />
    </div>
  );
}

function Main() {
  const { nav } = useNav();
  const { approvals } = useData();
  const waiting = approvals.data?.length ?? 0;
  const ask = askScreen(nav);
  const key =
    nav.section === 'ask' ? `ask:${nav.ask.length}:${ask.kind}:${ask.kind === 'answer' ? ask.conversationId : ''}` : nav.section;

  return (
    <>
      {/* The element is made here, with its props: a screen leaving keeps showing what it showed. */}
      <AnimatePresence initial={false}>{sectionView(nav, key)}</AnimatePresence>
      <TabBar waiting={waiting} />
      <AnimatePresence>{nav.drawer && <Drawer key="menu" waiting={waiting} />}</AnimatePresence>
      <AnimatePresence>{nav.reader && <Reader key={`${nav.reader.path}:${nav.reader.page ?? 0}`} target={nav.reader} />}</AnimatePresence>
      <AnimatePresence>
        {nav.sheet && <ApprovalSheet key={`${nav.sheet.sessionId}:${nav.sheet.stepId}`} target={nav.sheet} />}
      </AnimatePresence>
    </>
  );
}

function sectionView(nav: Nav, key: string) {
  switch (nav.section) {
    case 'ask': {
      const screen = askScreen(nav);
      if (screen.kind === 'conversations') return <Conversations key={key} filter={screen.filter ?? 'all'} />;
      if (screen.kind === 'answer')
        return <Ask key={key} conversationId={screen.conversationId} question={screen.question} workspaceId={screen.workspaceId} />;
      return <Home key={key} />;
    }
    case 'library':
      return <Library key={key} />;
    case 'approvals':
      return <Approvals key={key} />;
    case 'settings':
      return <Settings key={key} />;
    case 'workspaces':
      return <Workspaces key={key} />;
    default:
      return <ComingSoon key={key} section={nav.section} />;
  }
}
