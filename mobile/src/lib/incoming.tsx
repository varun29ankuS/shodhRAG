/**
 * What comes in from outside the app or from the Library's Add and the
 * camera: files to send (the Send sheet) and text or a link to ask about
 * ("Ask about this"). Android's side keeps what was shared until the app
 * takes it, so a share that opened the app is read once it starts.
 */
import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from 'react';
import { useNote } from './context';
import type { Incoming, Picked } from './device';
import { useDevice } from './queue';

export interface Shared {
  text: string;
  subject: string | null;
}

interface Offers {
  /** Files waiting for the user to choose where they go. */
  sending: Picked[] | null;
  /** Text or a link waiting to be asked about. */
  asking: Shared | null;
  /** The event whose brief a notification opened. */
  briefing: string | null;
  closeBrief: () => void;
  /** The event "Open in Shodh" opened, from the phone's calendar. */
  opened: string | null;
  closeOpened: () => void;
  offerSend: (files: Picked[]) => void;
  offerAsk: (shared: Shared) => void;
  closeSend: () => void;
  closeAsk: () => void;
}

const OffersContext = createContext<Offers | null>(null);

export function IncomingProvider({ children }: { children: ReactNode }) {
  const { files } = useDevice();
  const { show } = useNote();
  const [sending, setSending] = useState<Picked[] | null>(null);
  const [asking, setAsking] = useState<Shared | null>(null);
  const [briefing, setBriefing] = useState<string | null>(null);
  const [opened, setOpened] = useState<string | null>(null);

  const route = useCallback(
    (incoming: Incoming) => {
      switch (incoming.kind) {
        case 'files':
          if (incoming.files.length > 0) setSending(incoming.files);
          break;
        case 'text':
          setAsking({ text: incoming.text, subject: incoming.subject });
          break;
        case 'error':
          show(incoming.message, 'problem');
          break;
        case 'brief':
          setBriefing(incoming.eventId);
          break;
        case 'event':
          setOpened(incoming.id);
          break;
        case 'none':
          break;
      }
    },
    [show],
  );

  // Listen first, then take what arrived before the app was ready.
  useEffect(() => {
    let alive = true;
    let stop: (() => void) | null = null;
    const take = () => {
      files
        .takeIncoming()
        .then(incoming => {
          if (alive) route(incoming);
        })
        .catch(() => undefined);
    };
    files
      .onIncoming(take)
      .then(unlisten => {
        if (alive) stop = unlisten;
        else unlisten();
      })
      .catch(() => undefined)
      .finally(() => {
        if (alive) take();
      });
    return () => {
      alive = false;
      stop?.();
    };
  }, [files, route]);

  const value = useMemo<Offers>(
    () => ({
      sending,
      asking,
      briefing,
      closeBrief: () => setBriefing(null),
      opened,
      closeOpened: () => setOpened(null),
      offerSend: list => setSending(list),
      offerAsk: shared => setAsking(shared),
      closeSend: () => setSending(null),
      closeAsk: () => setAsking(null),
    }),
    [sending, asking, briefing, opened],
  );
  return <OffersContext.Provider value={value}>{children}</OffersContext.Provider>;
}

export function useOffers(): Offers {
  const offers = useContext(OffersContext);
  if (!offers) throw new Error('useOffers outside IncomingProvider');
  return offers;
}
