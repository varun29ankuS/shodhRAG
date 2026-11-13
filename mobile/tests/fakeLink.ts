import type { LinkTransport } from '../src/lib/invoke';

/** A link to a scripted computer: it records calls and sends events on demand. */
export class FakeLink implements LinkTransport {
  calls: Array<{ command: string; args: Record<string, unknown> }> = [];
  subscriptions: string[][] = [];
  attachments = 0;
  private handlers: Array<(name: string, payload: unknown) => void> = [];

  constructor(private readonly answer: (command: string, args: Record<string, unknown>) => Promise<unknown>) {}

  call(command: string, args: Record<string, unknown>): Promise<unknown> {
    this.calls.push({ command, args });
    return this.answer(command, args);
  }

  async subscribe(sessionIds: string[]): Promise<void> {
    this.subscriptions.push(sessionIds);
  }

  async onEvent(handler: (name: string, payload: unknown) => void): Promise<() => void> {
    this.attachments++;
    this.handlers.push(handler);
    return () => {
      this.handlers = this.handlers.filter(h => h !== handler);
    };
  }

  /** The computer sends an event. */
  send(name: string, payload: unknown) {
    for (const handler of [...this.handlers]) handler(name, payload);
  }
}
