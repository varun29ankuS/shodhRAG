/**
 * Finding K (UI structure audit, 2026-11-23): Ask cannot build a page or an app, only
 * Creator can. The prompt says so (`ASSISTANT_INSTRUCTIONS`, crates/shodh-rag), and when
 * a question asks for one, its answer carries a "Build in Creator" card that hands the
 * user's own words to a new Creator chat. This says which questions ask for one.
 *
 * Pure, for the Node tests.
 */

/** A verb that asks for something to be made. */
const MAKE = /\b(?:build|make|create|generate|code|write|design|put together)\b/i;
/** Something Creator makes: a page, a site, an app, a game, a small tool. */
const THING = /\b(?:web ?page|page|website|web ?site|site|landing page|web app|app|application|game|dashboard|widget|prototype|html|calculator|tool|timer|quiz)\b/i;
/** A question about how something is made is an explanation, not a request to make it. */
const EXPLAIN = /^\s*(?:how|why|what|when|where|which|who|explain|describe|tell me (?:how|why|what)|can you explain|is it|are there|does|do)\b/i;
/** The longest gap, in words, between the verb and the thing ("build me a small html page"). */
const GAP_WORDS = 6;

/** Whether `question` asks for a page, app or game to be built (not how to build one). */
export function asksToBuild(question: string): boolean {
  const text = question.replace(/\s+/g, ' ').trim();
  if (!text || EXPLAIN.test(text)) return false;
  const verb = MAKE.exec(text);
  if (!verb) return false;
  const after = text.slice(verb.index + verb[0].length).split(' ').filter(Boolean).slice(0, GAP_WORDS + 2).join(' ');
  return THING.test(after);
}
