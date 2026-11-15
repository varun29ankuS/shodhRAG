/**
 * Two-way sync between Shodh's calendar (on the computer) and the phone's own
 * calendar (Android's CalendarContract, mobile/plugins/shodh-android), as a
 * pure plan: given what each side holds now and what the last sync agreed
 * (the links, each with the version both sides had then), what to change on
 * each side. The app applies the plan (lib/agenda.tsx) and keeps the state
 * sealed on the phone.
 *
 * - Shodh's events go into Shodh's own calendar on the phone ("Shodh", in the
 *   elephant's red-orange), each with an "Open in Shodh" link.
 * - Events of the calendars the user chose go to the computer only while "Let
 *   Shodh use my calendar" is on; turning it off removes the copies there, so
 *   they are not indexed and the assistant does not see them. Calendars the
 *   user did not choose are never read (the app's Rust side refuses them too).
 * - Edits flow both ways. When both sides changed one event, the later edit
 *   wins (the computer's time of change; on the phone, the time the change was
 *   first seen) and the other version is kept in the history.
 * - A delete on either side is not passed on until the user says so: it waits
 *   as a question ("Remove it from your phone's calendar too?").
 */

/** An event as both sides compare it: the computer's wall-clock strings. */
export interface Fields {
  title: string;
  /** `YYYY-MM-DD` (all day) or `YYYY-MM-DDTHH:MM`, local time. */
  start: string;
  end: string | null;
  allDay: boolean;
  location: string | null;
  description: string;
}

/** An event on the computer (`load_events`). */
export interface DesktopEvent {
  id: string;
  title: string;
  description?: string;
  startTime: string;
  endTime?: string | null;
  allDay?: boolean;
  location?: string | null;
  color?: string | null;
  source?: string;
  sourceRef?: string | null;
  createdAt?: string;
  updatedAt?: string | null;
}

/** An event in the phone's calendar (the plugin's `calendarEvents`). */
export interface PhoneEvent {
  eventId: number;
  calendarId: number;
  title: string;
  startMs: number;
  endMs: number | null;
  allDay: boolean;
  location: string | null;
  description: string;
  /** The "Open in Shodh" link of an event Shodh put there. */
  shodhUri: string | null;
}

/** One event the last sync agreed on. */
export interface SyncLink {
  desktopId: string;
  calendarId: number;
  eventId: number;
  /** `shodh`: Shodh's event (or one added to Shodh's calendar on the phone); `phone`: an event of a chosen phone calendar. */
  origin: 'shodh' | 'phone';
  base: Fields;
  /** When the phone's change since `base` was first seen. */
  phoneChangedAt: number | null;
}

/** A version that lost to a later edit. */
export interface Lost {
  desktopId: string;
  side: 'phone' | 'computer';
  fields: Fields;
  at: number;
}

/** A delete waiting for the user to say whether the other side removes it too. */
export interface PendingDelete {
  id: string;
  deletedOn: 'phone' | 'computer';
  desktopId: string;
  calendarId: number;
  eventId: number;
  title: string;
}

export interface SyncState {
  links: SyncLink[];
  history: Lost[];
  pending: PendingDelete[];
}

export interface SyncOptions {
  /** Shodh's own calendar on the phone. */
  shodhCalendar: number | null;
  /** The phone calendars the user chose. */
  chosen: number[];
  /** "Let Shodh use my calendar". */
  useWithShodh: boolean;
  now: number;
  /** The span of the phone's calendar that was read: events outside it are left as they are. */
  from: number;
  to: number;
}

export type Action =
  | { kind: 'phoneInsert'; calendarId: number; desktopId: string; fields: Fields }
  | { kind: 'phoneUpdate'; calendarId: number; eventId: number; desktopId: string; origin: 'shodh' | 'phone'; fields: Fields }
  | { kind: 'phoneDelete'; calendarId: number; eventId: number }
  | { kind: 'desktopCreate'; calendarId: number; eventId: number; origin: 'shodh' | 'phone'; sourceRef: string; fields: Fields }
  | { kind: 'desktopUpdate'; id: string; fields: Fields }
  | { kind: 'desktopDelete'; id: string };

/** The most versions kept in the history. */
export const MAX_HISTORY = 100;

/** The reference of a computer copy of a phone calendar's event. */
export const MIRROR_PREFIX = 'phone-calendar:';

/** The "Open in Shodh" link Shodh puts on its events in the phone's calendar. */
export const EVENT_LINK = 'shodh://event/';

export function emptyState(): SyncState {
  return { links: [], history: [], pending: [] };
}

const pad = (n: number) => String(n).padStart(2, '0');

function localDate(d: Date): string {
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

function utcDate(ms: number): string {
  const d = new Date(ms);
  return `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())}`;
}

function localDateTime(ms: number): string {
  const d = new Date(ms);
  return `${localDate(d)}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

const DAY = 86_400_000;

/** A phone event as the computer would hold it. All-day events are UTC midnights on Android. */
/** A timed event without an end lasts an hour, as Android's calendar keeps it. */
function withEnd(fields: Fields): Fields {
  if (fields.allDay || fields.end !== null) return fields;
  return { ...fields, end: localDateTime(parseLocal(fields.start) + 3_600_000) };
}

export function phoneFields(event: PhoneEvent): Fields {
  const start = event.allDay ? utcDate(event.startMs) : localDateTime(event.startMs);
  let end: string | null = null;
  if (event.endMs !== null) {
    end = event.allDay ? utcDate(event.endMs - DAY) : localDateTime(event.endMs);
    if (event.allDay && end <= start) end = null;
  }
  return withEnd({
    title: event.title.trim(),
    start,
    end,
    allDay: event.allDay,
    location: event.location?.trim() ? event.location.trim() : null,
    description: event.description.trim(),
  });
}

function moment(value: string, allDay: boolean): string {
  const v = value.trim();
  return allDay ? v.slice(0, 10) : v.includes('T') || v.includes(' ') ? v.replace(' ', 'T').slice(0, 16) : `${v.slice(0, 10)}T00:00`;
}

/** A computer event as both sides compare it. */
export function desktopFields(event: DesktopEvent): Fields {
  const allDay = event.allDay === true;
  const end = event.endTime ? moment(event.endTime, allDay) : null;
  const start = moment(event.startTime, allDay);
  return withEnd({
    title: event.title.trim(),
    start,
    end: end !== null && allDay && end <= start ? null : end,
    allDay,
    location: event.location?.trim() ? event.location.trim() : null,
    description: (event.description ?? '').trim(),
  });
}

/** Whether an event starts inside the span of the phone's calendar that was read. */
export function inSpan(fields: Fields, from: number, to: number): boolean {
  const start = fields.allDay ? parseUtcDate(fields.start) : parseLocal(fields.start);
  return start >= from && start < to;
}

export function sameFields(a: Fields, b: Fields): boolean {
  return (
    a.title === b.title &&
    a.start === b.start &&
    a.end === b.end &&
    a.allDay === b.allDay &&
    a.location === b.location &&
    a.description === b.description
  );
}

function parseLocal(value: string): number {
  const [date, time = '00:00'] = value.split('T');
  const [y, m, d] = date.split('-').map(Number);
  const [h, mi] = time.split(':').map(Number);
  return new Date(y, m - 1, d, h || 0, mi || 0).getTime();
}

function parseUtcDate(value: string): number {
  const [y, m, d] = value.slice(0, 10).split('-').map(Number);
  return Date.UTC(y, m - 1, d);
}

/** Fields as Android's calendar takes them (all-day: UTC midnights, the end the day after). */
export function toPhone(fields: Fields): { title: string; startMs: number; endMs: number | null; allDay: boolean; location: string | null; description: string } {
  if (fields.allDay) {
    const startMs = parseUtcDate(fields.start);
    const last = fields.end ? parseUtcDate(fields.end) : startMs;
    return { title: fields.title, startMs, endMs: Math.max(last, startMs) + DAY, allDay: true, location: fields.location, description: fields.description };
  }
  return {
    title: fields.title,
    startMs: parseLocal(fields.start),
    endMs: fields.end ? parseLocal(fields.end) : null,
    allDay: false,
    location: fields.location,
    description: fields.description,
  };
}

/** The computer's arguments for `create_event` / `update_event`. */
export function toDesktop(fields: Fields): Record<string, unknown> {
  return {
    title: fields.title,
    startTime: fields.start,
    ...(fields.end ? { endTime: fields.end } : {}),
    allDay: fields.allDay,
    description: fields.description,
    ...(fields.location ? { location: fields.location } : {}),
  };
}

/** `update_event`'s arguments: the fields, and the optional ones cleared. */
export function desktopPatch(id: string, fields: Fields): Record<string, unknown> {
  const clear: string[] = [];
  if (!fields.end) clear.push('end_time');
  if (!fields.location) clear.push('location');
  if (!fields.description) clear.push('description');
  const { description, ...rest } = toDesktop(fields);
  return { id, ...rest, ...(fields.description ? { description } : {}), ...(clear.length > 0 ? { clear } : {}) };
}

const phoneKey = (calendarId: number, eventId: number) => `${calendarId}:${eventId}`;

function changedAt(event: DesktopEvent): number {
  const when = Date.parse(event.updatedAt ?? event.createdAt ?? '');
  return Number.isNaN(when) ? 0 : when;
}

/** Record when the phone's changes were first seen (run on every change of the phone's calendar, also while the computer is away). */
export function observe(state: SyncState, phone: PhoneEvent[], now: number): SyncState {
  const byKey = new Map(phone.map(e => [phoneKey(e.calendarId, e.eventId), e]));
  let changed = false;
  const links = state.links.map(link => {
    const event = byKey.get(phoneKey(link.calendarId, link.eventId));
    if (!event || link.phoneChangedAt !== null || sameFields(phoneFields(event), link.base)) return link;
    changed = true;
    return { ...link, phoneChangedAt: now };
  });
  return changed ? { ...state, links } : state;
}

function remember(history: Lost[], lost: Lost): Lost[] {
  return [...history, lost].slice(-MAX_HISTORY);
}

/** Whether a computer event is the computer's copy of a phone calendar's event. */
export function isMirror(event: DesktopEvent): boolean {
  return (event.sourceRef ?? '').startsWith(MIRROR_PREFIX);
}

/** What to change on each side now. */
export function plan(state: SyncState, desktop: DesktopEvent[], phone: PhoneEvent[], options: SyncOptions): { actions: Action[]; state: SyncState } {
  const reachable = new Set([...options.chosen, ...(options.shodhCalendar !== null ? [options.shodhCalendar] : [])]);
  // Only the calendars Shodh may read; any other event is left out entirely.
  const phoneEvents = phone.filter(e => reachable.has(e.calendarId));
  const byId = new Map(desktop.map(e => [e.id, e]));
  const byKey = new Map(phoneEvents.map(e => [phoneKey(e.calendarId, e.eventId), e]));
  const actions: Action[] = [];
  let history = state.history;
  const pending = [...state.pending];
  const links: SyncLink[] = [];
  const linkedDesktop = new Set<string>();
  const linkedPhone = new Set<string>();

  const ask = (link: SyncLink, deletedOn: PendingDelete['deletedOn'], title: string) => {
    if (!pending.some(p => p.desktopId === link.desktopId && p.eventId === link.eventId)) {
      pending.push({ id: `${link.desktopId}|${link.calendarId}|${link.eventId}`, deletedOn, desktopId: link.desktopId, calendarId: link.calendarId, eventId: link.eventId, title });
    }
  };

  for (const link of state.links) {
    const d = byId.get(link.desktopId);
    const p = byKey.get(phoneKey(link.calendarId, link.eventId));
    if (link.origin === 'phone' && !options.useWithShodh) {
      // "Let Shodh use my calendar" is off: the computer's copy goes, without asking (it is a copy).
      if (d) actions.push({ kind: 'desktopDelete', id: d.id });
      continue;
    }
    if (!reachable.has(link.calendarId)) {
      // The calendar is no longer chosen: the computer keeps Shodh's events; copies of phone events go.
      if (d && link.origin === 'phone') actions.push({ kind: 'desktopDelete', id: d.id });
      continue;
    }
    if (!inSpan(link.base, options.from, options.to)) {
      // Outside what was read: neither side is looked at, so nothing is taken as deleted.
      links.push(link);
      linkedDesktop.add(link.desktopId);
      linkedPhone.add(phoneKey(link.calendarId, link.eventId));
      continue;
    }
    if (!d && !p) continue;
    linkedDesktop.add(link.desktopId);
    linkedPhone.add(phoneKey(link.calendarId, link.eventId));
    if (!d && p) {
      ask(link, 'computer', p.title);
      links.push(link);
      continue;
    }
    if (d && !p) {
      ask(link, 'phone', d.title);
      links.push(link);
      continue;
    }
    if (!d || !p) continue;
    const ours = desktopFields(d);
    const theirs = phoneFields(p);
    const computerChanged = !sameFields(ours, link.base);
    const phoneChanged = !sameFields(theirs, link.base);
    if (!computerChanged && !phoneChanged) {
      links.push(link);
      continue;
    }
    if (computerChanged && !phoneChanged) {
      actions.push({ kind: 'phoneUpdate', calendarId: link.calendarId, eventId: link.eventId, desktopId: d.id, origin: link.origin, fields: ours });
      links.push({ ...link, base: ours, phoneChangedAt: null });
      continue;
    }
    if (!computerChanged && phoneChanged) {
      actions.push({ kind: 'desktopUpdate', id: d.id, fields: theirs });
      links.push({ ...link, base: theirs, phoneChangedAt: null });
      continue;
    }
    if (sameFields(ours, theirs)) {
      links.push({ ...link, base: ours, phoneChangedAt: null });
      continue;
    }
    // Both changed: the later edit wins; the other is kept in the history.
    const phoneAt = link.phoneChangedAt ?? options.now;
    if (changedAt(d) >= phoneAt) {
      actions.push({ kind: 'phoneUpdate', calendarId: link.calendarId, eventId: link.eventId, desktopId: d.id, origin: link.origin, fields: ours });
      history = remember(history, { desktopId: d.id, side: 'phone', fields: theirs, at: options.now });
      links.push({ ...link, base: ours, phoneChangedAt: null });
    } else {
      actions.push({ kind: 'desktopUpdate', id: d.id, fields: theirs });
      history = remember(history, { desktopId: d.id, side: 'computer', fields: ours, at: options.now });
      links.push({ ...link, base: theirs, phoneChangedAt: null });
    }
  }

  // Shodh's own events, new to the phone: into Shodh's calendar. Copies of phone
  // events are never put back on the phone.
  if (options.shodhCalendar !== null) {
    for (const d of desktop) {
      if (linkedDesktop.has(d.id) || isMirror(d) || !inSpan(desktopFields(d), options.from, options.to)) continue;
      const shown = phoneEvents.find(e => e.calendarId === options.shodhCalendar && e.shodhUri === `${EVENT_LINK}${d.id}`);
      if (shown) {
        // Already there (the sync's state was lost): linked again, as it is now.
        links.push({ desktopId: d.id, calendarId: shown.calendarId, eventId: shown.eventId, origin: 'shodh', base: phoneFields(shown), phoneChangedAt: null });
        linkedPhone.add(phoneKey(shown.calendarId, shown.eventId));
        linkedDesktop.add(d.id);
        continue;
      }
      actions.push({ kind: 'phoneInsert', calendarId: options.shodhCalendar, desktopId: d.id, fields: desktopFields(d) });
    }
  }

  // Phone events new to Shodh.
  for (const p of phoneEvents) {
    const key = phoneKey(p.calendarId, p.eventId);
    if (linkedPhone.has(key)) continue;
    if (p.calendarId === options.shodhCalendar) {
      if (p.shodhUri?.startsWith(EVENT_LINK)) continue; // its event is gone from the computer: asked about above, or left
      // Added to Shodh's calendar on the phone: it is the user's Shodh event.
      actions.push({ kind: 'desktopCreate', calendarId: p.calendarId, eventId: p.eventId, origin: 'shodh', sourceRef: `phone:cal-${p.calendarId}-${p.eventId}`, fields: phoneFields(p) });
      continue;
    }
    if (!options.useWithShodh) continue;
    const mirror = desktop.find(d => d.sourceRef === `${MIRROR_PREFIX}${p.calendarId}:${p.eventId}`);
    if (mirror) {
      links.push({ desktopId: mirror.id, calendarId: p.calendarId, eventId: p.eventId, origin: 'phone', base: desktopFields(mirror), phoneChangedAt: null });
      continue;
    }
    actions.push({ kind: 'desktopCreate', calendarId: p.calendarId, eventId: p.eventId, origin: 'phone', sourceRef: `${MIRROR_PREFIX}${p.calendarId}:${p.eventId}`, fields: phoneFields(p) });
  }

  // Without "Let Shodh use my calendar", no copy of a phone event stays on the computer.
  if (!options.useWithShodh) {
    const going = new Set(actions.flatMap(a => (a.kind === 'desktopDelete' ? [a.id] : [])));
    for (const d of desktop) if (isMirror(d) && !going.has(d.id)) actions.push({ kind: 'desktopDelete', id: d.id });
  }

  // A question about an event both sides no longer have is dropped.
  const stillLinked = new Set(links.map(l => `${l.desktopId}|${l.calendarId}|${l.eventId}`));
  return { actions, state: { links, history, pending: pending.filter(p => stillLinked.has(p.id)) } };
}

/** The user answered a delete: remove it on the other side too, or bring it back. */
export function resolveDelete(state: SyncState, id: string, remove: boolean, desktop: DesktopEvent[], phone: PhoneEvent[]): { actions: Action[]; state: SyncState } {
  const question = state.pending.find(p => p.id === id);
  const link = state.links.find(l => `${l.desktopId}|${l.calendarId}|${l.eventId}` === id);
  const rest: SyncState = { ...state, pending: state.pending.filter(p => p.id !== id), links: state.links.filter(l => l !== link) };
  if (!question || !link) return { actions: [], state: rest };
  if (question.deletedOn === 'phone') {
    const d = desktop.find(e => e.id === link.desktopId);
    if (!d) return { actions: [], state: rest };
    return remove
      ? { actions: [{ kind: 'desktopDelete', id: d.id }], state: rest }
      : { actions: [{ kind: 'phoneInsert', calendarId: link.calendarId, desktopId: d.id, fields: desktopFields(d) }], state: rest };
  }
  const p = phone.find(e => e.calendarId === link.calendarId && e.eventId === link.eventId);
  if (!p) return { actions: [], state: rest };
  return remove
    ? { actions: [{ kind: 'phoneDelete', calendarId: p.calendarId, eventId: p.eventId }], state: rest }
    : {
        actions: [
          {
            kind: 'desktopCreate',
            calendarId: p.calendarId,
            eventId: p.eventId,
            origin: link.origin,
            sourceRef: link.origin === 'phone' ? `${MIRROR_PREFIX}${p.calendarId}:${p.eventId}` : `phone:cal-${p.calendarId}-${p.eventId}`,
            fields: phoneFields(p),
          },
        ],
        state: rest,
      };
}

/** A link made once an action created the other side's event. */
export function withLink(state: SyncState, link: SyncLink): SyncState {
  return { ...state, links: [...state.links.filter(l => l.desktopId !== link.desktopId), link] };
}
