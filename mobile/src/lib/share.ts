/**
 * "Ask about this": text or a link shared to Shodh from another app. A link
 * goes in the question's own words (the user chose it, so the assistant may
 * open it without asking, as for a link they typed); any other text is
 * attached as someone else's words, which the computer fences, marks as
 * private for the web tools and never learns from. The question is answered
 * from the whole library.
 */
import type { Attached } from './ask';
import type { AskContext } from './nav';

/** The web addresses in `text`. */
export function linksIn(text: string): string[] {
  return [...text.matchAll(/https?:\/\/[^\s<>"')\]]+/gi)].map(m => m[0].replace(/[.,;:!?]+$/, ''));
}

/** Whether what was shared is a link (and nothing much besides it). */
export function sharedLink(text: string): string | null {
  const links = linksIn(text);
  if (links.length !== 1) return null;
  const rest = text.replace(links[0], '').trim();
  return rest.length <= 200 ? links[0] : null;
}

/** The question asked about what was shared: its words, and what it carries. */
export function sharedQuestion(question: string, shared: string): { text: string; context: AskContext } {
  const asked = question.trim() || 'What does my library say about this?';
  const link = sharedLink(shared);
  if (link) {
    const rest = shared.replace(link, '').trim();
    const attached: Attached[] = rest ? [{ origin: 'shared', text: rest }] : [];
    return { text: `${asked}\n\n${link}`, context: { attached, passages: [] } };
  }
  return { text: asked, context: { attached: [{ origin: 'shared', text: shared.trim() }], passages: [] } };
}
