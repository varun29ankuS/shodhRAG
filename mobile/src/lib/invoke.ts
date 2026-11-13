/**
 * The phone's `invoke` and `listen`: the desktop's own Tauri commands and
 * events, carried over the phone link. A desktop screen reused on the phone
 * calls `invoke('agent_send', …)` and `listen('agent_event', …)` exactly as on
 * the desktop; the shim sends the call to the computer and routes the
 * computer's events back to the listeners of each event name.
 *
 * What may be called is decided by the phone link's allowlist, in Rust: the
 * phone's side refuses anything off the list before sending it, and the
 * computer checks it again. Errors come back as the computer's window would
 * receive them (`{ code, message }`; `code` is `refused` for a call the phone
 * may not make and `offline` when the computer is not connected).
 */

/** What the phone's link does for the shim. */
export interface LinkTransport {
  /** Run one of the computer's commands; rejects with the computer's error. */
  call(command: string, args: Record<string, unknown>): Promise<unknown>;
  /** Receive the agent events of these sessions only (replaces the earlier list). */
  subscribe(sessionIds: string[]): Promise<void>;
  /** Receive every event the computer sends; resolves to a function that stops it. */
  onEvent(handler: (name: string, payload: unknown) => void): Promise<() => void>;
}

/** An event as Tauri's `listen` hands it to a handler. */
export interface LinkEvent<T> {
  event: string;
  id: number;
  payload: T;
}

export type Unlisten = () => void;

export interface Shim {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(event: string, handler: (event: LinkEvent<T>) => void): Promise<Unlisten>;
  subscribe(sessionIds: string[]): Promise<void>;
}

type Handler = (event: LinkEvent<unknown>) => void;

export function createShim(transport: LinkTransport): Shim {
  const listeners = new Map<string, Set<Handler>>();
  let nextId = 1;
  // One subscription to the link for all listeners, made with the first one.
  let attached: Promise<() => void> | null = null;

  const route = (name: string, payload: unknown) => {
    const handlers = listeners.get(name);
    if (!handlers) return;
    const event: LinkEvent<unknown> = { event: name, id: nextId++, payload };
    // A copy: a handler may stop listening while the event is delivered.
    for (const handler of [...handlers]) handler(event);
  };

  return {
    invoke<T>(command: string, args: Record<string, unknown> = {}) {
      return transport.call(command, args) as Promise<T>;
    },

    async listen<T>(name: string, handler: (event: LinkEvent<T>) => void) {
      const typed = handler as Handler;
      const handlers = listeners.get(name) ?? new Set<Handler>();
      listeners.set(name, handlers);
      handlers.add(typed);
      try {
        attached ??= transport.onEvent(route);
        await attached;
      } catch (error) {
        attached = null;
        handlers.delete(typed);
        if (handlers.size === 0) listeners.delete(name);
        throw error;
      }
      return () => {
        handlers.delete(typed);
        if (handlers.size === 0 && listeners.get(name) === handlers) listeners.delete(name);
      };
    },

    subscribe(sessionIds: string[]) {
      return transport.subscribe(sessionIds);
    },
  };
}
