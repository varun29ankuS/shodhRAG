/**
 * The phone itself in tests: an outbox kept in memory with the same steps as
 * the app's sealed one (crates/shodh-link/src/outbox.rs), and an updater that
 * answers as scripted. `MemoryOutbox` keeps its items in a shared list, so a
 * test can "kill" the app (drop the sender) and start it again on the same
 * queue.
 */
import type { Device, FileAdded, Incoming, PhoneFiles, Picked, PhotoRead, UpdateCheck } from '../src/lib/device';
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

export interface MemoryDevice extends Device {
  outbox: MemoryOutbox;
  files: MemoryFiles;
  checks: number;
  installs: number;
}

export function memoryDevice(options: { update?: UpdateCheck; install?: () => Promise<boolean> } = {}): MemoryDevice {
  const device: MemoryDevice = {
    outbox: new MemoryOutbox(),
    files: new MemoryFiles(),
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
