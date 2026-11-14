/**
 * Calendar sync between Shodh (the computer) and the phone's own calendar
 * (src/lib/calsync.ts), and the offline copy and queued edits of tasks.
 */
import { describe, expect, it } from 'vitest';
import { applyActions } from '../src/lib/agenda';
import {
  desktopFields,
  emptyState,
  observe,
  phoneFields,
  plan,
  resolveDelete,
  toPhone,
  type DesktopEvent,
  type PhoneEvent,
  type SyncOptions,
  type SyncState,
} from '../src/lib/calsync';
import { createShim } from '../src/lib/invoke';
import { OutboxSender } from '../src/lib/outbox';
import { Subscriptions } from '../src/lib/subscriptions';
import { memoryDevice, MemoryOutbox } from './device';
import { FakeLink } from './fakeLink';

const SHODH = 9;
const PERSONAL = 1;
const WORK = 2;
const NOW = new Date(2026, 10, 14, 8, 0).getTime();
const at = (day: number, hour: number, minute = 0) => new Date(2026, 10, day, hour, minute).getTime();

const options = (extra: Partial<SyncOptions> = {}): SyncOptions => ({
  shodhCalendar: SHODH,
  chosen: [PERSONAL],
  useWithShodh: true,
  now: NOW,
  from: NOW - 30 * 86_400_000,
  to: NOW + 365 * 86_400_000,
  ...extra,
});

function phoneEvent(calendarId: number, eventId: number, title: string, start: number, extra: Partial<PhoneEvent> = {}): PhoneEvent {
  return { eventId, calendarId, title, startMs: start, endMs: start + 3_600_000, allDay: false, location: null, description: '', shodhUri: null, ...extra };
}

/** The computer: its events, kept as its own create, update and delete commands keep them. */
class Computer {
  events: DesktopEvent[] = [];
  private next = 0;
  private clock = NOW;
  link = new FakeLink(async (command, args) => {
    const stamp = new Date((this.clock += 1000)).toISOString();
    switch (command) {
      case 'create_event': {
        const made: DesktopEvent = {
          id: `e${++this.next}`,
          title: args.title as string,
          startTime: args.startTime as string,
          endTime: (args.endTime as string) ?? null,
          allDay: args.allDay as boolean,
          description: (args.description as string) ?? '',
          location: (args.location as string) ?? null,
          source: 'phone',
          sourceRef: args.sourceRef as string,
          createdAt: stamp,
          updatedAt: stamp,
        };
        this.events.push(made);
        return made;
      }
      case 'update_event': {
        const event = this.events.find(e => e.id === args.id)!;
        Object.assign(event, {
          title: args.title,
          startTime: args.startTime,
          endTime: (args.endTime as string) ?? null,
          allDay: args.allDay,
          description: (args.description as string) ?? '',
          location: (args.location as string) ?? null,
          updatedAt: stamp,
        });
        return event;
      }
      case 'delete_event':
        this.events = this.events.filter(e => e.id !== args.id);
        return true;
      case 'load_events':
        return this.events;
      case 'load_tasks':
        return [];
      case 'create_task':
        return { id: 't1', ...args };
      case 'update_task':
        return { id: args.id, status: args.status };
      default:
        throw new Error(`unexpected ${command}`);
    }
  });
  shim = createShim(this.link);
  /** The user edits an event in Shodh on the computer at `when`. */
  edit(id: string, change: Partial<DesktopEvent>, when: number) {
    Object.assign(this.events.find(e => e.id === id)!, change, { updatedAt: new Date(when).toISOString() });
  }
}

async function syncOnce(computer: Computer, device: ReturnType<typeof memoryDevice>, state: SyncState, extra: Partial<SyncOptions> = {}) {
  const planned = plan(state, computer.events, await device.calendar.events(), options(extra));
  const next = await applyActions(planned.actions, planned.state, computer.shim, device);
  return { actions: planned.actions, state: next };
}

async function syncOn(device: ReturnType<typeof memoryDevice>, extra: Partial<{ useWithShodh: boolean; chosen: number[] }> = {}) {
  await device.calendar.choose({ enabled: true, chosen: extra.chosen ?? [PERSONAL], shodhCalendar: null, useWithShodh: extra.useWithShodh ?? true });
}

describe('calendar sync', () => {
  it('a_phone_event_appears_in_shodh_and_back', async () => {
    const computer = new Computer();
    const device = memoryDevice();
    await syncOn(device);
    device.calendar.stored.push(phoneEvent(PERSONAL, 41, 'Dentist', at(16, 10)));

    // The phone's event appears in Shodh, marked as the phone calendar's copy.
    let { state } = await syncOnce(computer, device, emptyState());
    expect(computer.events).toHaveLength(1);
    expect(computer.events[0]).toMatchObject({ title: 'Dentist', startTime: '2026-11-16T10:00', sourceRef: 'phone-calendar:1:41', source: 'phone' });
    // The computer's copy is never put back on the phone.
    expect(device.calendar.writes.filter(w => w.kind === 'insert')).toEqual([]);

    // Moved in Shodh: the phone's calendar follows.
    computer.edit(computer.events[0].id, { startTime: '2026-11-16T11:30', endTime: '2026-11-16T12:30' }, NOW + 60_000);
    ({ state } = await syncOnce(computer, device, state));
    const dentist = device.calendar.stored.find(e => e.eventId === 41)!;
    expect(phoneFields(dentist)).toMatchObject({ start: '2026-11-16T11:30', end: '2026-11-16T12:30' });

    // And Shodh's own event goes into the phone's Shodh calendar, with its "Open in Shodh" link.
    computer.events.push({ id: 'shodh-1', title: 'Reading group', startTime: '2026-11-15T17:00', endTime: '2026-11-15T18:00', allDay: false, source: 'user', createdAt: new Date(NOW).toISOString() });
    ({ state } = await syncOnce(computer, device, state));
    const mirrored = device.calendar.stored.find(e => e.calendarId === SHODH)!;
    expect(mirrored).toMatchObject({ title: 'Reading group', shodhUri: 'shodh://event/shodh-1' });
    // Nothing more to do: both sides agree.
    expect((await syncOnce(computer, device, state)).actions).toEqual([]);
  });

  it('a_conflict_keeps_the_latest_and_the_history', async () => {
    const computer = new Computer();
    const device = memoryDevice();
    await syncOn(device);
    computer.events.push({ id: 'm1', title: 'Meet Asha', startTime: '2026-11-17T09:00', endTime: '2026-11-17T10:00', allDay: false, source: 'user', createdAt: new Date(NOW).toISOString() });
    let { state } = await syncOnce(computer, device, emptyState());
    const onPhone = device.calendar.stored.find(e => e.calendarId === SHODH)!;

    // The phone's edit is seen at 08:05 (while the computer was asleep); the computer's is at 08:10.
    onPhone.title = 'Meet Asha (phone)';
    state = observe(state, await device.calendar.events(), NOW + 5 * 60_000);
    computer.edit('m1', { title: 'Meet Asha (computer)' }, NOW + 10 * 60_000);
    ({ state } = await syncOnce(computer, device, state, { now: NOW + 20 * 60_000 }));
    expect(onPhone.title).toBe('Meet Asha (computer)');
    expect(computer.events[0].title).toBe('Meet Asha (computer)');
    expect(state.history).toEqual([expect.objectContaining({ desktopId: 'm1', side: 'phone', fields: expect.objectContaining({ title: 'Meet Asha (phone)' }) })]);

    // The other way round: the phone's edit is the later one.
    computer.edit('m1', { title: 'Asha at noon' }, NOW + 30 * 60_000);
    onPhone.title = 'Asha at one';
    state = observe(state, await device.calendar.events(), NOW + 40 * 60_000);
    ({ state } = await syncOnce(computer, device, state, { now: NOW + 41 * 60_000 }));
    expect(computer.events[0].title).toBe('Asha at one');
    expect(state.history.at(-1)).toMatchObject({ side: 'computer', fields: expect.objectContaining({ title: 'Asha at noon' }) });
  });

  it('unselected_calendars_never_leave_the_phone', async () => {
    const computer = new Computer();
    const device = memoryDevice();
    await syncOn(device, { chosen: [PERSONAL] });
    device.calendar.stored.push(phoneEvent(WORK, 7, 'Board meeting', at(18, 14)), phoneEvent(PERSONAL, 8, 'Yoga', at(18, 7)));
    // Even if an event of another calendar reached the plan, it is left out.
    const planned = plan(emptyState(), [], device.calendar.stored, options());
    expect(planned.actions.map(a => (a.kind === 'desktopCreate' ? a.fields.title : a.kind))).toEqual(['Yoga']);
    await syncOnce(computer, device, emptyState());
    expect(computer.events.map(e => e.title)).toEqual(['Yoga']);
  });

  it('calendar_events_are_not_indexed_without_consent', async () => {
    const computer = new Computer();
    const device = memoryDevice();
    await syncOn(device, { useWithShodh: false });
    device.calendar.stored.push(phoneEvent(PERSONAL, 8, 'Yoga', at(18, 7)));
    // Off: shown on the phone, never sent to the computer (where events are indexed).
    let { state, actions } = await syncOnce(computer, device, emptyState(), { useWithShodh: false });
    expect(actions.filter(a => a.kind.startsWith('desktop'))).toEqual([]);
    expect(computer.events).toEqual([]);

    // On, then off again: the computer's copy goes, Shodh's own events stay.
    ({ state } = await syncOnce(computer, device, state, { useWithShodh: true }));
    computer.events.push({ id: 'own', title: 'Reading group', startTime: '2026-11-15T17:00', allDay: false, source: 'user', createdAt: new Date(NOW).toISOString() });
    ({ state } = await syncOnce(computer, device, state, { useWithShodh: true }));
    expect(computer.events.map(e => e.title).sort()).toEqual(['Reading group', 'Yoga']);
    ({ state, actions } = await syncOnce(computer, device, state, { useWithShodh: false }));
    expect(computer.events.map(e => e.title)).toEqual(['Reading group']);
    expect(state.links.every(l => l.origin === 'shodh')).toBe(true);
    // A copy the sync no longer knows of goes too.
    computer.events.push({ id: 'stray', title: 'Old copy', startTime: '2026-11-19', allDay: true, sourceRef: 'phone-calendar:1:99' });
    await syncOnce(computer, device, state, { useWithShodh: false });
    expect(computer.events.map(e => e.id)).toEqual(['own']);
  });

  it('a delete on either side asks first', async () => {
    const computer = new Computer();
    const device = memoryDevice();
    await syncOn(device);
    computer.events.push({ id: 'm1', title: 'Meet Asha', startTime: '2026-11-17T09:00', allDay: false, createdAt: new Date(NOW).toISOString() });
    let { state } = await syncOnce(computer, device, emptyState());
    // Removed on the phone: the computer keeps it until the user says.
    device.calendar.stored = device.calendar.stored.filter(e => e.calendarId !== SHODH);
    let actions;
    ({ state, actions } = await syncOnce(computer, device, state));
    expect(actions).toEqual([]);
    expect(state.pending).toEqual([expect.objectContaining({ deletedOn: 'phone', title: 'Meet Asha' })]);
    // "Keep it": it comes back on the phone. Asked again, and "Remove": gone on the computer too.
    const kept = resolveDelete(state, state.pending[0].id, false, computer.events, device.calendar.stored);
    state = await applyActions(kept.actions, kept.state, computer.shim, device);
    expect(device.calendar.stored.some(e => e.calendarId === SHODH && e.title === 'Meet Asha')).toBe(true);
    device.calendar.stored = device.calendar.stored.filter(e => e.calendarId !== SHODH);
    ({ state } = await syncOnce(computer, device, state));
    const removed = resolveDelete(state, state.pending[0].id, true, computer.events, device.calendar.stored);
    await applyActions(removed.actions, removed.state, computer.shim, device);
    expect(computer.events).toEqual([]);
  });

  it('times read the same on both sides', () => {
    const allDay = phoneEvent(PERSONAL, 1, 'Holiday', Date.UTC(2026, 10, 20), { allDay: true, endMs: Date.UTC(2026, 10, 21) });
    expect(phoneFields(allDay)).toMatchObject({ start: '2026-11-20', end: null, allDay: true });
    expect(toPhone(phoneFields(allDay))).toMatchObject({ startMs: Date.UTC(2026, 10, 20), endMs: Date.UTC(2026, 10, 21) });
    const timed = desktopFields({ id: 'x', title: ' Call ', startTime: '2026-11-20T09:15:00', allDay: false });
    expect(timed).toEqual({ title: 'Call', start: '2026-11-20T09:15', end: '2026-11-20T10:15', allDay: false, location: null, description: '' });
    expect(phoneFields(phoneEvent(PERSONAL, 2, 'Call', toPhone(timed).startMs, { endMs: toPhone(timed).endMs }))).toEqual(timed);
  });
});

describe('tasks while the computer is away', () => {
  it('offline_edits_sync_after_wake', async () => {
    const computer = new Computer();
    const outbox = new MemoryOutbox();
    const ref = 'phone:item-1';
    await outbox.add('item-1', { kind: 'edit', command: 'create_task', args: { title: 'Call Asha', sourceRef: ref }, label: 'Add the task “Call Asha”' });
    await outbox.add('item-2', { kind: 'edit', command: 'update_task', args: { id: 't0', status: 'completed' }, label: 'Tick off “Read Self-RAG”' });
    const sent: string[] = [];
    const link = new FakeLink(async (command, args) => {
      if (command === 'load_tasks') return [];
      sent.push(command);
      return computer.link.call(command, args);
    });
    const shim = createShim(link);
    // The computer wakes: the edits go in the order they were made.
    await new OutboxSender(outbox, { desktop: shim, subscriptions: new Subscriptions(ids => shim.subscribe(ids)) }).run(new AbortController().signal);
    expect(sent).toEqual(['create_task', 'update_task']);
    expect(outbox.items.map(i => i.state)).toEqual(['answered', 'answered']);
  });

  it('a task made just before a kill is not made twice', async () => {
    const outbox = new MemoryOutbox();
    await outbox.add('item-1', { kind: 'edit', command: 'create_task', args: { title: 'Call Asha', sourceRef: 'phone:item-1' }, label: 'Add the task' });
    await outbox.mark('item-1', 'sending');
    const calls: string[] = [];
    const link = new FakeLink(async command => {
      calls.push(command);
      if (command === 'load_tasks') return [{ id: 't1', title: 'Call Asha', status: 'pending', sourceRef: 'phone:item-1' }];
      throw new Error(`unexpected ${command}`);
    });
    const shim = createShim(link);
    await new OutboxSender(outbox, { desktop: shim, subscriptions: new Subscriptions(ids => shim.subscribe(ids)) }).run(new AbortController().signal);
    expect(calls).toEqual(['load_tasks']);
    expect(outbox.items[0].state).toBe('answered');
  });
});
