/**
 * The phone's outbox in the app: what waits for the computer, adding to it,
 * and sending it in order whenever the computer is connected (lib/outbox.ts).
 */
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { useLink } from './context';
import type { Device } from './device';
import type { LinkStatus } from './link';
import { finished, OutboxSender, type QueuedItem, type SenderDeps, type Work } from './outbox';

export interface Queue {
  items: QueuedItem[];
  /** Keep `work` to send when the computer is back (at once when it is connected), as item `id` (new when absent). */
  add(work: Work, id?: string): Promise<QueuedItem>;
  remove(id: string): Promise<void>;
  /** Items not finished yet. */
  waiting: number;
}

const QueueContext = createContext<Queue | null>(null);
const DeviceContext = createContext<Device | null>(null);

/** Whether a question may be kept for later: the phone is paired and the computer is only away. */
export function canQueue(status: LinkStatus): boolean {
  return status.state === 'offline' || status.state === 'connecting';
}

export function DeviceProvider({ device, children }: { device: Device; children: ReactNode }) {
  return <DeviceContext.Provider value={device}>{children}</DeviceContext.Provider>;
}

export function useDevice(): Device {
  const device = useContext(DeviceContext);
  if (!device) throw new Error('useDevice outside DeviceProvider');
  return device;
}

/** A queued file sent to the library; one the computer did not add fails with its reason. */
export function fileSender(files: Device['files']): NonNullable<SenderDeps['sendFile']> {
  return async item => {
    const answer = await files.send(item.work);
    if (!answer.added) throw { code: 'refused', message: answer.reason ?? 'Your computer did not add it.' };
  };
}

export function QueueProvider({ children }: { children: ReactNode }) {
  const { connected, desktop, subscriptions } = useLink();
  const { outbox, files } = useDevice();
  const [items, setItems] = useState<QueuedItem[]>([]);
  const sender = useMemo(
    () => new OutboxSender(outbox, { desktop, subscriptions, sendFile: fileSender(files) }),
    [outbox, desktop, subscriptions, files],
  );
  const running = useRef<AbortController | null>(null);

  const reload = useCallback(() => {
    outbox
      .list()
      .then(setItems)
      .catch(() => {
        // The queue cannot be read now; it is read again on its next change.
      });
  }, [outbox]);

  // Listen first, then read, so no change is missed.
  useEffect(() => {
    let alive = true;
    let stop: (() => void) | null = null;
    outbox
      .onChanged(reload)
      .then(unlisten => {
        if (alive) stop = unlisten;
        else unlisten();
      })
      .catch(() => undefined)
      .finally(() => {
        if (alive) reload();
      });
    return () => {
      alive = false;
      stop?.();
    };
  }, [outbox, reload]);

  const kick = useCallback(() => {
    const controller = running.current;
    if (controller && !controller.signal.aborted) sender.run(controller.signal).catch(() => undefined);
  }, [sender]);

  // Sent while the computer is connected; a new connection starts again where it stopped.
  useEffect(() => {
    if (!connected) return;
    const controller = new AbortController();
    running.current = controller;
    sender.run(controller.signal).catch(() => undefined);
    return () => {
      controller.abort();
      if (running.current === controller) running.current = null;
    };
  }, [connected, sender]);

  const add = useCallback(
    async (work: Work, id: string = crypto.randomUUID()) => {
      const item = await outbox.add(id, work);
      reload();
      kick();
      return item;
    },
    [outbox, reload, kick],
  );
  const remove = useCallback((id: string) => outbox.remove(id).then(reload), [outbox, reload]);
  const waiting = items.filter(i => !finished(i.state)).length;
  const value = useMemo<Queue>(() => ({ items, add, remove, waiting }), [items, add, remove, waiting]);
  return <QueueContext.Provider value={value}>{children}</QueueContext.Provider>;
}

export function useQueue(): Queue {
  const queue = useContext(QueueContext);
  if (!queue) throw new Error('useQueue outside QueueProvider');
  return queue;
}
