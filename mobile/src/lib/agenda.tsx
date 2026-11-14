/**
 * Tasks and the agenda on the phone: read from the computer while it is
 * connected (its own task and calendar commands, on the link's allowlist),
 * kept as a sealed offline copy on the phone, and shown from that copy while
 * the computer is away. The user's own edits made then wait in the outbox and
 * are sent when it wakes.
 *
 * Calendar sync (opt-in, Settings): Shodh's events go into the phone's own
 * "Shodh" calendar and edits flow both ways (lib/calsync.ts). Events of the
 * phone calendars the user chose are shown here, marked with where they come
 * from; they go to the computer only with "Let Shodh use my calendar".
 */
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import {
  desktopFields,
  desktopPatch,
  emptyState,
  EVENT_LINK,
  isMirror,
  observe,
  plan,
  resolveDelete,
  toDesktop,
  toPhone,
  withLink,
  type Action,
  type DesktopEvent,
  type PendingDelete,
  type PhoneEvent,
  type SyncState,
} from './calsync';
import { useLink } from './context';
import type { CalendarChoice, Device } from './device';
import type { Shim } from './invoke';
import { errorText } from './link';
import { useDevice } from './queue';
import { useRemote } from './remote';

/** A task on the computer (`load_tasks`). */
export interface Task {
  id: string;
  title: string;
  description?: string;
  dueDate?: string | null;
  priority?: string;
  status: string;
  source?: string;
  sourceRef?: string | null;
  updatedAt?: string;
}

/** The computer's tasks or calendar changed. */
export const CALENDAR_CHANGED = 'calendar-changed';

/** What the offline copy keeps. */
interface AgendaCopy {
  tasks: Task[];
  events: DesktopEvent[];
  savedAt: number;
}

/** How far back and ahead the phone's calendar is read. */
const PAST = 30 * 86_400_000;
const AHEAD = 365 * 86_400_000;

export const NO_CHOICE: CalendarChoice = { enabled: false, chosen: [], shodhCalendar: null, useWithShodh: false };

export interface Agenda {
  tasks: Task[] | null;
  events: DesktopEvent[] | null;
  /** Shown from the sealed copy: the computer is away. */
  fromCopy: boolean;
  savedAt: number | null;
  /** Events of the chosen phone calendars that are not Shodh's (shown, marked). */
  phoneEvents: PhoneEvent[];
  choice: CalendarChoice;
  setChoice: (choice: CalendarChoice) => Promise<void>;
  /** Deletes waiting for the user's answer. */
  pending: PendingDelete[];
  answer: (id: string, remove: boolean) => Promise<void>;
  syncError: string | null;
  reload: () => void;
}

const AgendaContext = createContext<Agenda | null>(null);

const loadAll = (desktop: Shim) =>
  Promise.all([desktop.invoke<Task[]>('load_tasks'), desktop.invoke<DesktopEvent[]>('load_events')]).then(([tasks, events]) => ({ tasks, events }));

const AGENDA_EVENTS = [CALENDAR_CHANGED];

/** Apply a sync plan's actions, one at a time, linking what each one makes. */
export async function applyActions(actions: Action[], state: SyncState, desktop: Shim, device: Device): Promise<SyncState> {
  let next = state;
  for (const action of actions) {
    switch (action.kind) {
      case 'phoneInsert': {
        const made = await device.calendar.write({
          kind: 'insert',
          calendarId: action.calendarId,
          fields: { ...toPhone(action.fields), shodhUri: `${EVENT_LINK}${action.desktopId}` },
        });
        if (typeof made.eventId === 'number') {
          next = withLink(next, { desktopId: action.desktopId, calendarId: action.calendarId, eventId: made.eventId, origin: 'shodh', base: action.fields, phoneChangedAt: null });
        }
        break;
      }
      case 'phoneUpdate':
        await device.calendar.write({
          kind: 'update',
          calendarId: action.calendarId,
          eventId: action.eventId,
          // Only Shodh's own events carry "Open in Shodh"; the user's calendar events are left as they are.
          fields: action.origin === 'shodh' ? { ...toPhone(action.fields), shodhUri: `${EVENT_LINK}${action.desktopId}` } : toPhone(action.fields),
        });
        break;
      case 'phoneDelete':
        await device.calendar.write({ kind: 'delete', calendarId: action.calendarId, eventId: action.eventId });
        break;
      case 'desktopCreate': {
        const made = await desktop.invoke<DesktopEvent>('create_event', { ...toDesktop(action.fields), sourceRef: action.sourceRef });
        next = withLink(next, { desktopId: made.id, calendarId: action.calendarId, eventId: action.eventId, origin: action.origin, base: desktopFields(made), phoneChangedAt: null });
        break;
      }
      case 'desktopUpdate':
        await desktop.invoke('update_event', desktopPatch(action.id, action.fields));
        break;
      case 'desktopDelete':
        await desktop.invoke('delete_event', { id: action.id });
        break;
    }
  }
  return next;
}

export function AgendaProvider({ children }: { children: ReactNode }) {
  const { connected, desktop } = useLink();
  const device = useDevice();
  const remote = useRemote('agenda', loadAll, AGENDA_EVENTS);
  const [copy, setCopy] = useState<AgendaCopy | null>(null);
  const [choice, setChoiceState] = useState<CalendarChoice>(NO_CHOICE);
  const [phoneEvents, setPhoneEvents] = useState<PhoneEvent[]>([]);
  const [sync, setSync] = useState<SyncState>(emptyState());
  const [syncError, setSyncError] = useState<string | null>(null);
  const [phoneVersion, setPhoneVersion] = useState(0);
  const running = useRef<Promise<void>>(Promise.resolve());

  // The sealed copy and the calendar choice, once.
  useEffect(() => {
    let alive = true;
    device.sealed
      .read<AgendaCopy>('agenda')
      .then(saved => {
        if (alive && saved) setCopy(saved);
      })
      .catch(() => undefined);
    device.calendar
      .choice()
      .then(saved => {
        if (alive) setChoiceState(saved);
      })
      .catch(() => undefined);
    device.sealed
      .read<SyncState>('calendar-sync')
      .then(saved => {
        if (alive && saved) setSync(saved);
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [device]);

  // Each fresh read is kept as the offline copy.
  useEffect(() => {
    if (!remote.data) return;
    const saved: AgendaCopy = { ...remote.data, savedAt: Date.now() };
    setCopy(saved);
    device.sealed.write('agenda', saved).catch(() => undefined);
  }, [remote.data, device]);

  // The phone's calendars changed (while the app runs).
  useEffect(() => {
    if (!choice.enabled) return;
    let stop: (() => void) | null = null;
    let alive = true;
    device.calendar
      .onChanged(() => setPhoneVersion(v => v + 1))
      .then(unlisten => {
        if (alive) stop = unlisten;
        else unlisten();
      })
      .catch(() => undefined);
    return () => {
      alive = false;
      stop?.();
    };
  }, [choice.enabled, device]);

  // Read the phone's chosen calendars; note what changed there; sync while connected.
  useEffect(() => {
    if (!choice.enabled) {
      setPhoneEvents([]);
      return;
    }
    const desktopEvents = remote.data?.events ?? null;
    running.current = running.current.then(async () => {
      try {
        const now = Date.now();
        const phone = await device.calendar.events(now - PAST, now + AHEAD);
        setPhoneEvents(phone);
        let state = (await device.sealed.read<SyncState>('calendar-sync')) ?? emptyState();
        state = observe(state, phone, now);
        if (connected && desktopEvents) {
          const planned = plan(state, desktopEvents, phone, {
            shodhCalendar: choice.shodhCalendar,
            chosen: choice.chosen,
            useWithShodh: choice.useWithShodh,
            now,
            from: now - PAST,
            to: now + AHEAD,
          });
          state = await applyActions(planned.actions, planned.state, desktop, device);
        }
        await device.sealed.write('calendar-sync', state);
        setSync(state);
        setSyncError(null);
      } catch (e) {
        setSyncError(errorText(e));
      }
    });
  }, [choice, connected, remote.data, phoneVersion, desktop, device]);

  // Calendar sync off, or "Let Shodh use my calendar" off: no copy of a phone
  // event stays on the computer (it would be indexed and read by the assistant).
  const sends = choice.enabled && choice.useWithShodh;
  useEffect(() => {
    if (sends || !connected || !remote.data) return;
    const copies = remote.data.events.filter(isMirror);
    if (copies.length === 0) return;
    running.current = running.current.then(async () => {
      try {
        for (const event of copies) await desktop.invoke('delete_event', { id: event.id });
        const state = (await device.sealed.read<SyncState>('calendar-sync')) ?? emptyState();
        const kept = { ...state, links: state.links.filter(l => l.origin !== 'phone') };
        await device.sealed.write('calendar-sync', kept);
        setSync(kept);
      } catch (e) {
        setSyncError(errorText(e));
      }
    });
  }, [sends, connected, remote.data, desktop, device]);

  const setChoice = useCallback(
    async (next: CalendarChoice) => {
      if (next.enabled && !(await device.calendar.access())) {
        throw new Error('Shodh may not use this phone’s calendar. Allow it in Android’s settings for Shodh.');
      }
      setChoiceState(await device.calendar.choose(next));
    },
    [device],
  );

  const answer = useCallback(
    async (id: string, remove: boolean) => {
      const desktopEvents = remote.data?.events ?? [];
      const resolved = resolveDelete(sync, id, remove, desktopEvents, phoneEvents);
      const state = await applyActions(resolved.actions, resolved.state, desktop, device);
      await device.sealed.write('calendar-sync', state);
      setSync(state);
      remote.reload();
    },
    [sync, remote, phoneEvents, desktop, device],
  );

  const live = connected && remote.data !== undefined;
  const shownPhone = useMemo(() => {
    const own = new Set(sync.links.map(l => `${l.calendarId}:${l.eventId}`));
    // Shodh's own calendar and the phone events already on the computer show once, as Shodh's.
    return phoneEvents.filter(e => e.calendarId !== choice.shodhCalendar && !own.has(`${e.calendarId}:${e.eventId}`));
  }, [phoneEvents, sync.links, choice.shodhCalendar]);

  const value = useMemo<Agenda>(
    () => ({
      tasks: live ? remote.data!.tasks : (copy?.tasks ?? null),
      events: live ? remote.data!.events : (copy?.events ?? null),
      fromCopy: !live && copy !== null,
      savedAt: copy?.savedAt ?? null,
      phoneEvents: shownPhone,
      choice,
      setChoice,
      pending: sync.pending,
      answer,
      syncError,
      reload: remote.reload,
    }),
    [live, remote.data, remote.reload, copy, shownPhone, choice, setChoice, sync.pending, answer, syncError],
  );
  return <AgendaContext.Provider value={value}>{children}</AgendaContext.Provider>;
}

export function useAgenda(): Agenda {
  const agenda = useContext(AgendaContext);
  if (!agenda) throw new Error('useAgenda outside AgendaProvider');
  return agenda;
}
