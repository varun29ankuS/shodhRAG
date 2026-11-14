/**
 * What runs behind the screens: the push address given to the computer
 * (UnifiedPush), and the alarms of meeting briefs. A brief is scheduled only
 * for one of Shodh's events in the next day that has one (nothing for events
 * nothing matches), only with "Let Shodh use my calendar", and only when the
 * user chose how many minutes before.
 */
import { useEffect, useState } from 'react';
import { useAgenda } from '../lib/agenda';
import { briefsAllowed, loadBrief } from '../lib/brief';
import { desktopFields, toPhone } from '../lib/calsync';
import { useLink } from '../lib/context';
import { useOffers } from '../lib/incoming';
import { useDevice } from '../lib/queue';
import { AnimatePresence } from 'framer-motion';
import { BriefSheet } from './BriefSheet';

/** The phone's own settings that are not the computer's. */
export interface PhoneSettings {
  /** Minutes before a meeting its brief is offered; null: no briefs. */
  briefMinutes: number | null;
}

export const NO_SETTINGS: PhoneSettings = { briefMinutes: null };

/** Sent on the window when the phone's settings were saved. */
export const SETTINGS_CHANGED = 'shodh:settings';

const DAY = 86_400_000;
/** The most briefs looked for at once. */
const MAX_BRIEFS = 8;

export function Background() {
  const { connected, desktop } = useLink();
  const { notifications, files, sealed } = useDevice();
  const agenda = useAgenda();
  const offers = useOffers();
  const [settings, setSettings] = useState<PhoneSettings>(NO_SETTINGS);

  useEffect(() => {
    const read = () => {
      sealed
        .read<PhoneSettings>('settings')
        .then(saved => setSettings({ ...NO_SETTINGS, ...(saved ?? {}) }))
        .catch(() => undefined);
    };
    read();
    window.addEventListener(SETTINGS_CHANGED, read);
    return () => window.removeEventListener(SETTINGS_CHANGED, read);
  }, [sealed]);

  // The push address reaches the computer when it is connected, and again when the push app hands a new one.
  useEffect(() => {
    if (!connected) return;
    const sync = () => {
      notifications
        .mode()
        .then(mode => (mode === 'push' ? notifications.syncPush() : false))
        .catch(() => undefined);
    };
    sync();
    let stop: (() => void) | null = null;
    let alive = true;
    files
      .onIncoming(sync)
      .then(unlisten => {
        if (alive) stop = unlisten;
        else unlisten();
      })
      .catch(() => undefined);
    return () => {
      alive = false;
      stop?.();
    };
  }, [connected, notifications, files]);

  // Briefs before the next day's meetings.
  const allowed = briefsAllowed(agenda.choice) && settings.briefMinutes !== null;
  useEffect(() => {
    if (!allowed) {
      notifications.scheduleBriefs([]).catch(() => undefined);
      return;
    }
    if (!connected || !agenda.events) return;
    const now = Date.now();
    const minutes = settings.briefMinutes ?? 10;
    const soon = agenda.events
      .filter(e => !(e.sourceRef ?? '').startsWith('phone-calendar:'))
      .map(e => ({ id: e.id, at: toPhone(desktopFields(e)).startMs - minutes * 60_000, allDay: e.allDay === true }))
      .filter(e => !e.allDay && e.at > now && e.at < now + DAY)
      .slice(0, MAX_BRIEFS);
    let alive = true;
    Promise.all(soon.map(e => loadBrief(desktop, e.id).then(brief => (brief ? ([e.id, e.at] as [string, number]) : null)).catch(() => null)))
      .then(found => {
        if (alive) return notifications.scheduleBriefs(found.filter((f): f is [string, number] => f !== null));
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [allowed, connected, agenda.events, settings.briefMinutes, desktop, notifications]);

  const event = offers.briefing ? agenda.events?.find(e => e.id === offers.briefing) : undefined;
  return (
    <AnimatePresence>
      {offers.briefing && <BriefSheet key="brief" eventId={offers.briefing} title={event?.title ?? 'your meeting'} onClose={offers.closeBrief} />}
    </AnimatePresence>
  );
}
