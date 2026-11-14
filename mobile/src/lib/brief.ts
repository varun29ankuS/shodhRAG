/**
 * "A brief before your meetings": for one event, the conversations and the
 * notes and documents about it, each cited, read from the computer
 * (`meeting_brief`, retrieval only). Offered only while calendar sync and "Let
 * Shodh use my calendar" are on; an event nothing matches has no brief.
 */
import type { CalendarChoice } from './device';
import type { Shim } from './invoke';

export interface BriefSource {
  n: number;
  file: string;
  path: string;
  page: string | null;
  text: string;
  kind: 'note' | 'document';
}

export interface Brief {
  eventId: string;
  title: string;
  conversations: Array<{ id: string; title: string; updatedAt: string }>;
  sources: BriefSource[];
}

/** Whether briefs may be asked for: they read the calendar, so only with the user's consent. */
export function briefsAllowed(choice: CalendarChoice): boolean {
  return choice.enabled && choice.useWithShodh;
}

export const loadBrief = (shim: Shim, eventId: string) => shim.invoke<Brief | null>('meeting_brief', { eventId });
