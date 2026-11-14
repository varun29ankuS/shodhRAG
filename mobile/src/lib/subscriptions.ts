/**
 * Which sessions' agent events the computer sends this phone. The link keeps
 * one list per connection and each subscribe replaces it, and the computer
 * forgets steps of sessions dropped from it (they can no longer be
 * answered). So every part of the app that needs a session's events (the
 * answer being read, the approvals listed) names its own sessions here, and
 * the link is sent the union of them all.
 */
export class Subscriptions {
  private readonly owners = new Map<string, string[]>();
  /** The union last sent, or null when the link must be told again. */
  private sent: string | null = null;

  constructor(private readonly send: (sessionIds: string[]) => Promise<void>) {}

  /** The sessions sent to the link: every owner's, once each, in order. */
  union(): string[] {
    const all = new Set<string>();
    for (const ids of this.owners.values()) for (const id of ids) all.add(id);
    return [...all].sort();
  }

  /** `owner` now needs these sessions (none: it needs nothing). */
  set(owner: string, sessionIds: readonly string[]): Promise<void> {
    if (sessionIds.length === 0) this.owners.delete(owner);
    else this.owners.set(owner, [...new Set(sessionIds)]);
    return this.flush();
  }

  /** After a new connection: the computer knows nothing yet, so the list is sent again. */
  reconnected(): Promise<void> {
    this.sent = null;
    return this.owners.size === 0 ? Promise.resolve() : this.flush();
  }

  private async flush(): Promise<void> {
    const union = this.union();
    const key = union.join('\n');
    if (key === this.sent) return;
    this.sent = key;
    try {
      await this.send(union);
    } catch (error) {
      // Not sent: the next change (or the next connection) sends it.
      if (this.sent === key) this.sent = null;
      throw error;
    }
  }
}
