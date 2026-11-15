/**
 * The app: the splash while the link starts, pairing when this phone has no
 * computer yet, and otherwise the sections (lib/nav.ts) with the tab bar,
 * the menu, the Reader and the approval sheet over them.
 */
import { useCallback, useEffect, useRef, useState } from 'react';
import { AnimatePresence, MotionConfig } from 'framer-motion';
import { Drawer } from './components/Drawer';
import { NoteView } from './components/NoteView';
import { Splash } from './components/Splash';
import { TabBar } from './components/TabBar';
import { LinkProvider, NavProvider, NoteProvider, ThemeProvider, MenuShown, useLink, useNav, useThemeChoice } from './lib/context';
import { DataProvider, useData } from './lib/data';
import type { LinkSource } from './lib/link';
import { tauriDevice, type Device } from './lib/device';
import { useSystemInsets } from './lib/insets';
import { DeviceProvider, QueueProvider } from './lib/queue';
import { IncomingProvider, useOffers } from './lib/incoming';
import { SendSheet } from './components/SendSheet';
import { AskAbout } from './components/AskAbout';
import { LockGate } from './components/LockGate';
import { Background } from './components/Background';
import { AgendaProvider } from './lib/agenda';
import { Tasks } from './screens/Tasks';
import { askScreen, type Nav } from './lib/nav';
import type { ThemeChoice } from './lib/theme';
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

/**
 * `theme` opens the app at that theme choice without saving it (CI's screenshot
 * tour, lib/tour.ts); otherwise the user's own choice applies.
 */
export function App({
  source,
  start = null,
  device = tauriDevice,
  theme,
}: {
  source: LinkSource;
  start?: Start | null;
  device?: Device;
  theme?: ThemeChoice;
}) {
  const [pairing, setPairing] = useState(start?.screen === 'pair');
  const openPairing = useCallback(() => setPairing(true), []);
  // Where Android's status bar and gesture bar are, for every edge of the app (theme.css --inset-*).
  useSystemInsets(device.insets);
  return (
    <MotionConfig reducedMotion="user">
      <ThemeProvider systemBars={device.systemBars} pinned={theme}>
        <DeviceProvider device={device}>
          <LockGate source={source} skip={start !== null}>
        <LinkProvider source={source} openPairing={openPairing}>
            <QueueProvider>
              <NavProvider initial={start?.nav ?? undefined}>
                <NoteProvider>
                  <IncomingProvider>
                    <DataProvider>
                      <AgendaProvider>
                        <Shell start={start} pairing={pairing} closePairing={() => setPairing(false)} />
                      </AgendaProvider>
                    </DataProvider>
                  </IncomingProvider>
                </NoteProvider>
              </NavProvider>
            </QueueProvider>
        </LinkProvider>
          </LockGate>
        </DeviceProvider>
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
  // The first screen shown, once: the line CI's launch test and screenshots wait for. It is
  // written once the screen is really there: the splash has gone and, for a screen
  // that opens with the menu, the menu has finished opening.
  const [splashGone, setSplashGone] = useState(false);
  const [menuShown, setMenuShown] = useState(false);
  const waitsForMenu = start?.nav?.drawer === true;
  const told = useRef(false);
  useEffect(() => {
    if (screen === null || told.current || !splashGone || (waitsForMenu && !menuShown)) return;
    told.current = true;
    source.ready(screen, theme).catch(() => {
      // The app logged why it is not ready, and the screen says it too.
    });
  }, [screen, theme, source, splashGone, waitsForMenu, menuShown]);

  return (
    <div className="app">
      {status.state === 'failed' ? (
        <Failed message={status.message} />
      ) : showPair ? (
        <Pair refusal={status.state === 'refused' ? status.message : null} onClose={unpaired && !pairing ? null : closePairing} />
      ) : (
        <MenuShown.Provider value={() => setMenuShown(true)}>
          <Main />
        </MenuShown.Provider>
      )}
      <AnimatePresence onExitComplete={() => setSplashGone(true)}>{status.state === 'starting' && <Splash key="splash" />}</AnimatePresence>
      <NoteView />
    </div>
  );
}

function Main() {
  const { nav } = useNav();
  const { approvals } = useData();
  const offers = useOffers();
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
      <AnimatePresence>{offers.sending && <SendSheet key="send" files={offers.sending} />}</AnimatePresence>
      <AnimatePresence>{offers.asking && <AskAbout key="ask-about" shared={offers.asking} />}</AnimatePresence>
      <Background />
    </>
  );
}

function sectionView(nav: Nav, key: string) {
  switch (nav.section) {
    case 'ask': {
      const screen = askScreen(nav);
      if (screen.kind === 'conversations') return <Conversations key={key} filter={screen.filter ?? 'all'} />;
      if (screen.kind === 'answer')
        return <Ask key={key} conversationId={screen.conversationId} question={screen.question} workspaceId={screen.workspaceId} context={screen.context} />;
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
    case 'tasks':
      return <Tasks key={key} />;
    default:
      return <ComingSoon key={key} section={nav.section} />;
  }
}
