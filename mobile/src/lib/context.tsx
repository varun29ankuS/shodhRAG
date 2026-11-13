/**
 * What every screen reads: the link (its state, the computer's commands and
 * events, the sessions subscribed), where the app is (lib/nav.ts), the
 * theme, and short notes ("Sent to your computer"). Tests give the app a
 * scripted computer through the same `LinkSource`.
 */
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { createShim, type Shim } from './invoke';
import { errorText, isConnected, type LinkSource, type LinkStatus } from './link';
import { canGoBack, initialNav, navigate, type Nav, type NavAction, type Section } from './nav';
import { Subscriptions } from './subscriptions';
import { useTheme, type ThemeState } from './theme';

export interface Linked {
  source: LinkSource;
  status: LinkStatus;
  connected: boolean;
  desktop: Shim;
  subscriptions: Subscriptions;
  /** Try the computer again; resolves when the attempt ended. */
  reconnect: () => Promise<void>;
  /** Show the pairing screen. */
  openPairing: () => void;
}

const LinkContext = createContext<Linked | null>(null);

export function useLink(): Linked {
  const linked = useContext(LinkContext);
  if (!linked) throw new Error('useLink outside LinkProvider');
  return linked;
}

/** The link's state, kept current: listen first, then read, so no change is missed. */
function useLinkStatus(source: LinkSource): LinkStatus {
  const [status, setStatus] = useState<LinkStatus>({ state: 'starting' });
  useEffect(() => {
    let alive = true;
    let stop: (() => void) | null = null;
    source
      .onStatus(next => {
        if (alive) setStatus(next);
      })
      .then(unlisten => {
        if (!alive) unlisten();
        else stop = unlisten;
        return source.status();
      })
      .then(current => {
        if (alive) setStatus(current);
      })
      .catch(error => {
        if (alive) setStatus({ state: 'failed', message: errorText(error) });
      });
    return () => {
      alive = false;
      stop?.();
    };
  }, [source]);
  return status;
}

export function LinkProvider({ source, openPairing, children }: { source: LinkSource; openPairing: () => void; children: ReactNode }) {
  const status = useLinkStatus(source);
  const desktop = useMemo(() => createShim(source.transport), [source]);
  const subscriptions = useMemo(() => new Subscriptions(ids => source.transport.subscribe(ids)), [source]);
  const connected = isConnected(status);

  // A new connection knows no subscriptions: send them again.
  useEffect(() => {
    if (connected) subscriptions.reconnected().catch(() => undefined);
  }, [connected, subscriptions]);

  const reconnect = useCallback(() => source.reconnect(), [source]);
  const value = useMemo<Linked>(
    () => ({ source, status, connected, desktop, subscriptions, reconnect, openPairing }),
    [source, status, connected, desktop, subscriptions, reconnect, openPairing],
  );
  return <LinkContext.Provider value={value}>{children}</LinkContext.Provider>;
}

// ── Where the app is ───────────────────────────────────────────────

export interface Navigation {
  nav: Nav;
  dispatch: (action: NavAction) => void;
  /** Back, as the phone's Back does it. */
  back: () => void;
}

const NavContext = createContext<Navigation | null>(null);

export function useNav(): Navigation {
  const navigation = useContext(NavContext);
  if (!navigation) throw new Error('useNav outside NavProvider');
  return navigation;
}

/**
 * Navigation, with the phone's Back: every step forward is a history entry,
 * so Android's Back (the button or the edge swipe) pops it and the app goes
 * back one step; at the start, Back leaves the app.
 */
export function NavProvider({ initial, children }: { initial?: Nav; children: ReactNode }) {
  const [nav, setNav] = useState<Nav>(() => initial ?? initialNav());
  const navRef = useRef(nav);
  navRef.current = nav;
  const pushed = useRef(0);

  const apply = useCallback((action: NavAction) => {
    setNav(current => navigate(current, action));
  }, []);

  useEffect(() => {
    const popped = () => {
      pushed.current = Math.max(0, pushed.current - 1);
      apply({ type: 'back' });
    };
    window.addEventListener('popstate', popped);
    return () => window.removeEventListener('popstate', popped);
  }, [apply]);

  const dispatch = useCallback(
    (action: NavAction) => {
      const before = navRef.current;
      const after = navigate(before, action);
      if (after === before) return;
      const forward = action.type !== 'back' && action.type !== 'crumb' && !(action.type === 'drawer' && !action.open) && !(action.type === 'sheet' && action.target === null);
      if (forward && canGoBack(after)) {
        window.history.pushState({ shodh: pushed.current + 1 }, '');
        pushed.current += 1;
      }
      apply(action);
    },
    [apply],
  );

  const back = useCallback(() => {
    if (pushed.current > 0) window.history.back();
    else apply({ type: 'back' });
  }, [apply]);

  const value = useMemo<Navigation>(() => ({ nav, dispatch, back }), [nav, dispatch, back]);
  return <NavContext.Provider value={value}>{children}</NavContext.Provider>;
}

/** Go to a section from the menu or the tab bar. */
export function useGo(): (section: Section) => void {
  const { dispatch } = useNav();
  return useCallback((section: Section) => dispatch({ type: 'go', section }), [dispatch]);
}

// ── Theme ──────────────────────────────────────────────────────────

const ThemeContext = createContext<ThemeState | null>(null);

export function ThemeProvider({ children }: { children: ReactNode }) {
  const theme = useTheme();
  return <ThemeContext.Provider value={theme}>{children}</ThemeContext.Provider>;
}

export function useThemeChoice(): ThemeState {
  const theme = useContext(ThemeContext);
  if (!theme) throw new Error('useThemeChoice outside ThemeProvider');
  return theme;
}

// ── Notes ──────────────────────────────────────────────────────────

export interface Note {
  id: number;
  text: string;
  tone: 'done' | 'problem';
}

const NoteContext = createContext<{ note: Note | null; show: (text: string, tone?: Note['tone']) => void } | null>(null);

export function NoteProvider({ children }: { children: ReactNode }) {
  const [note, setNote] = useState<Note | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const show = useCallback((text: string, tone: Note['tone'] = 'done') => {
    if (timer.current) clearTimeout(timer.current);
    setNote({ id: Date.now(), text, tone });
    timer.current = setTimeout(() => setNote(null), 3200);
  }, []);
  useEffect(() => () => {
    if (timer.current) clearTimeout(timer.current);
  }, []);
  const value = useMemo(() => ({ note, show }), [note, show]);
  return <NoteContext.Provider value={value}>{children}</NoteContext.Provider>;
}

export function useNote() {
  const notes = useContext(NoteContext);
  if (!notes) throw new Error('useNote outside NoteProvider');
  return notes;
}
