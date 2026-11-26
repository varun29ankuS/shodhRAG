/**
 * The phone itself in tests: an outbox kept in memory with the same steps as
 * the app's sealed one (crates/shodh-link/src/outbox.rs), and an updater that
 * answers as scripted. `MemoryOutbox` keeps its items in a shared list, so a
 * test can "kill" the app (drop the sender) and start it again on the same
 * queue.
 */
import type { CalendarChoice, CalendarWrite, Device, LockStatus, NotifyMode, PhoneLock, PhoneNotifications, PhoneVoice, FileAdded, Incoming, PhoneCalendar, PhoneCalendarInfo, PhoneFiles, Picked, PhotoRead, Sealed, SealedName, UpdateCheck } from '../src/lib/device';
import type { PhoneEvent } from '../src/lib/calsync';
import type { Insets, SystemInsets } from '../src/lib/insets';
import type { OutboxSource, QueuedFile, QueuedItem, QueueState, Work } from '../src/lib/outbox';

const STEPS: Record<QueueState, QueueState[]> = {
  waiting: ['sending', 'failed'],
  sending: ['waiting', 'sent', 'failed'],
  sent: ['answered', 'failed'],
  answered: [],
  failed: ['waiting'],
};

export class MemoryOutbox implements OutboxSource {
  /** Every change of state, in order: [id, state]. */
  history: Array<[string, QueueState]> = [];
  private handlers = new Set<() => void>();
  private clock = 0;

  constructor(public items: QueuedItem[] = []) {}

  async list() {
    return this.items.map(i => ({ ...i }));
  }

  async add(id: string, work: Work) {
    if (this.items.some(i => i.id === id)) throw new Error('This was already queued.');
    const item: QueuedItem = { id, queuedAt: ++this.clock, state: 'waiting', work, error: null, changedAt: this.clock };
    this.items.push(item);
    this.history.push([id, 'waiting']);
    this.changed();
    return { ...item };
  }

  async mark(id: string, state: QueueState, error: string | null = null) {
    const item = this.items.find(i => i.id === id);
    if (!item) throw new Error('This is no longer waiting for your computer.');
    if (item.state !== state) {
      if (!STEPS[item.state].includes(state)) throw new Error(`This cannot change from ${item.state} to ${state}.`);
      item.state = state;
      item.error = state === 'failed' ? error : null;
      item.changedAt = ++this.clock;
      this.history.push([id, state]);
      this.changed();
    }
    return { ...item };
  }

  async remove(id: string) {
    this.items = this.items.filter(i => i.id !== id);
    this.changed();
  }

  async onChanged(handler: () => void) {
    this.handlers.add(handler);
    return () => {
      this.handlers.delete(handler);
    };
  }

  private changed() {
    for (const handler of [...this.handlers]) handler();
  }
}

/** The phone's system bars in tests: their insets now, and a change sent to every listener. */
export class MemoryInsets implements SystemInsets {
  private handlers = new Set<(insets: Insets) => void>();

  constructor(public now: Insets = { top: 0, right: 0, bottom: 0, left: 0 }) {}

  async current() {
    return { ...this.now };
  }

  async onChanged(handler: (insets: Insets) => void) {
    this.handlers.add(handler);
    return () => {
      this.handlers.delete(handler);
    };
  }

  change(next: Insets) {
    this.now = next;
    for (const handler of [...this.handlers]) handler({ ...next });
  }
}

export interface MemoryDevice extends Device {
  /** Every theme the system bars were set to. */
  bars: Array<'light' | 'dark'>;
  outbox: MemoryOutbox;
  files: MemoryFiles;
  calendar: MemoryCalendar;
  sealed: MemorySealed;
  lock: MemoryLock;
  notifications: MemoryNotifications;
  voice: MemoryVoice;
  insets: MemoryInsets;
  checks: number;
  installs: number;
}

export function memoryDevice(options: { update?: UpdateCheck; install?: () => Promise<boolean>; insets?: Insets } = {}): MemoryDevice {
  const device: MemoryDevice = {
    outbox: new MemoryOutbox(),
    files: new MemoryFiles(),
    calendar: new MemoryCalendar(),
    sealed: new MemorySealed(),
    lock: new MemoryLock(),
    notifications: new MemoryNotifications(),
    voice: new MemoryVoice(),
    insets: new MemoryInsets(options.insets),
    bars: [],
    systemBars: async theme => {
      device.bars.push(theme);
    },
    checks: 0,
    installs: 0,
    updater: {
      check: async () => {
        device.checks += 1;
        return options.update ?? { state: 'unavailable', message: 'Shodh 0.0.11 for Android is not published yet.' };
      },
      install: async () => {
        device.installs += 1;
        return options.install ? options.install() : true;
      },
    },
  };
  return device;
}

/** The phone's files in tests: what the picker, the camera and the share sheet give, and what was sent. */
export class MemoryFiles implements PhoneFiles {
  picks: Picked[] = [];
  photo: Picked | null = null;
  incoming: Incoming = { kind: 'none' };
  sent: QueuedFile[] = [];
  photosSent: QueuedFile[] = [];
  discarded: string[] = [];
  /** The computer's answer to each file sent. */
  answer: (file: QueuedFile) => FileAdded = file => ({ name: file.name, path: `/data/From my phone/${file.name}`, added: true, reason: null, notice: null });
  photoText = 'Retrieval should be adaptive.';
  private handlers = new Set<() => void>();

  async pick() {
    return this.picks;
  }
  async takePhoto() {
    return this.photo;
  }
  async takeIncoming() {
    const incoming = this.incoming;
    this.incoming = { kind: 'none' };
    return incoming;
  }
  async onIncoming(handler: () => void) {
    this.handlers.add(handler);
    return () => {
      this.handlers.delete(handler);
    };
  }
  /** Another app shares to Shodh while it runs. */
  share(incoming: Incoming) {
    this.incoming = incoming;
    for (const handler of [...this.handlers]) handler();
  }
  async details(picked: Picked, workspaceId: string | null): Promise<QueuedFile> {
    return { kind: 'file', localPath: picked.path, name: picked.name, size: 1234, sha256: 'a'.repeat(64), workspaceId };
  }
  async discard(path: string) {
    this.discarded.push(path);
  }
  async send(file: QueuedFile) {
    this.sent.push(file);
    return this.answer(file);
  }
  async sendPhoto(file: QueuedFile): Promise<PhotoRead> {
    this.photosSent.push(file);
    return { photoId: 'p'.repeat(32), name: file.name, text: this.photoText, words: this.photoText.split(/\s+/).length };
  }
  async onProgress() {
    return () => undefined;
  }
}

/** The phone's sealed files in memory. */
export class MemorySealed implements Sealed {
  files = new Map<SealedName, unknown>();
  async read<T>(name: SealedName): Promise<T | null> {
    return (this.files.get(name) as T | undefined) ?? null;
  }
  async write(name: SealedName, value: unknown) {
    this.files.set(name, JSON.parse(JSON.stringify(value)));
  }
}

/** The phone's own calendar in memory: its calendars and events, and every write. */
export class MemoryCalendar implements PhoneCalendar {
  granted = true;
  calendars: PhoneCalendarInfo[] = [
    { id: 1, name: 'Personal', account: 'me@example.org', color: '#3366ff', shodh: false },
    { id: 2, name: 'Work', account: 'me@example.org', color: '#22aa66', shodh: false },
  ];
  /** Every event on the phone, in the calendars Shodh may and may not read. */
  stored: PhoneEvent[] = [];
  writes: CalendarWrite[] = [];
  current: CalendarChoice = { enabled: false, chosen: [], shodhCalendar: null, useWithShodh: false };
  private nextId = 100;
  private handlers = new Set<() => void>();

  async access() {
    return this.granted;
  }
  async list() {
    return this.calendars;
  }
  async choice() {
    return this.current;
  }
  async choose(choice: CalendarChoice) {
    const shodhCalendar = choice.enabled ? (this.current.shodhCalendar ?? 9) : this.current.shodhCalendar;
    if (choice.enabled && !this.calendars.some(c => c.shodh)) this.calendars.push({ id: 9, name: 'Shodh', account: 'Shodh', color: '#e8482a', shodh: true });
    this.current = { ...choice, shodhCalendar };
    return this.current;
  }
  async events() {
    const reachable = new Set([...(this.current.enabled ? this.current.chosen : []), ...(this.current.enabled && this.current.shodhCalendar !== null ? [this.current.shodhCalendar] : [])]);
    return this.stored.filter(e => reachable.has(e.calendarId)).map(e => ({ ...e }));
  }
  async write(write: CalendarWrite) {
    this.writes.push(write);
    if (write.kind === 'insert') {
      const eventId = ++this.nextId;
      const f = write.fields as { title: string; startMs: number; endMs: number | null; allDay: boolean; location: string | null; description: string; shodhUri?: string };
      this.stored.push({ eventId, calendarId: write.calendarId, title: f.title, startMs: f.startMs, endMs: f.endMs ?? f.startMs + 3_600_000, allDay: f.allDay, location: f.location, description: f.description, shodhUri: f.shodhUri ?? null });
      return { eventId };
    }
    if (write.kind === 'update') {
      const event = this.stored.find(e => e.eventId === write.eventId && e.calendarId === write.calendarId);
      const f = write.fields as { title: string; startMs: number; endMs: number | null; allDay: boolean; location: string | null; description: string };
      if (event) Object.assign(event, { ...f, endMs: f.endMs ?? f.startMs + 3_600_000 });
      return {};
    }
    this.stored = this.stored.filter(e => !(e.eventId === write.eventId && e.calendarId === write.calendarId));
    return {};
  }
  async onChanged(handler: () => void) {
    this.handlers.add(handler);
    return () => {
      this.handlers.delete(handler);
    };
  }
}

/** The lock in tests: the PIN is 482913; `status` says what it is now. */
export class MemoryLock implements PhoneLock {
  status_: LockStatus = { required: true, hasPin: true, biometric: false, biometry: 'fingerprint', locked: false };
  pin = '482913';
  async status() {
    return { ...this.status_ };
  }
  async setPin(pin: string) {
    if (!/^\d{4,12}$/.test(pin)) throw 'A PIN is 4 to 12 digits.';
    this.pin = pin;
    this.status_ = { ...this.status_, hasPin: true, locked: false };
  }
  async unlock(pin: string) {
    if (pin !== this.pin) throw 'That is not your PIN. 4 more tries before Shodh waits 30 seconds.';
    this.status_ = { ...this.status_, locked: false };
  }
  async unlockBiometric() {
    if (!this.status_.biometric) throw 'Use your PIN.';
    this.status_ = { ...this.status_, locked: false };
  }
  async configure(pin: string, change: { enabled?: boolean; biometric?: boolean }) {
    if (pin !== this.pin) throw 'That is not your PIN.';
    this.status_ = { ...this.status_, required: change.enabled ?? this.status_.required, biometric: change.biometric ?? this.status_.biometric };
  }
  async hidden() {}
  async shown() {
    return this.status_.locked;
  }
}

/** Notifications in tests: the mode, and what was asked of Android. */
export class MemoryNotifications implements PhoneNotifications {
  current: NotifyMode = 'off';
  sets: Array<[NotifyMode, string | undefined]> = [];
  pushApps = ['io.heckel.ntfy'];
  syncs = 0;
  briefs: Array<Array<[string, number]>> = [];
  async mode() {
    return this.current;
  }
  async set(mode: NotifyMode, distributor?: string) {
    this.sets.push([mode, distributor]);
    this.current = mode;
    return mode;
  }
  async distributors() {
    return this.pushApps;
  }
  async syncPush() {
    this.syncs += 1;
    return true;
  }
  async scheduleBriefs(briefs: Array<[string, number]>) {
    this.briefs.push(briefs);
  }
}

/** Voice in tests: what the user "says", and everything read aloud. */
export class MemoryVoice implements PhoneVoice {
  heard = 'What does Self-RAG retrieve?';
  spoken: string[] = [];
  async listen() {
    return this.heard;
  }
  async stopListening() {}
  async speak(text: string) {
    this.spoken.push(text);
  }
  async stop() {}
}
