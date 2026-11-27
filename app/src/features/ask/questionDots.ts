/**
 * The question dots rail beside a conversation (`QuestionRail.tsx`): one dot per question,
 * the dot of the question in view filled, the question's first line on hover. Hidden
 * below three questions, where there is nothing to jump between.
 *
 * Pure, so the Node tests load it (`app/tests/questionDots.test.ts`).
 */

/** The fewest questions the rail is shown for. */
export const RAIL_MIN_QUESTIONS = 3;

/** The most characters of a question its dot's label shows. */
export const RAIL_LABEL_CHARS = 80;

/** What the rail needs of a message. */
export interface RailMessage {
  role: string;
  content: string;
}

/** One dot: the question's index in the conversation, and its label. */
export interface RailQuestion {
  index: number;
  label: string;
}

/**
 * The question's first line that has text, its spaces collapsed, cut at a word to
 * `max` characters with an ellipsis. Cut here, not by the stylesheet, so the label is
 * always whole on screen.
 */
export function firstLine(text: string, max: number = RAIL_LABEL_CHARS): string {
  const line = text.split('\n').map(l => l.replace(/\s+/g, ' ').trim()).find(l => l.length > 0) ?? '';
  if (line.length <= max) return line;
  const cut = line.slice(0, max - 1);
  const space = cut.lastIndexOf(' ');
  // A cut at a word unless that leaves too little of the line.
  const kept = space >= max / 2 ? cut.slice(0, space) : cut;
  return `${kept.trimEnd()}…`;
}

/** The questions of a conversation, in order: every message the user wrote. */
export function railQuestions(messages: readonly RailMessage[]): RailQuestion[] {
  const out: RailQuestion[] = [];
  messages.forEach((message, index) => {
    if (message.role === 'user') out.push({ index, label: firstLine(message.content) || 'Question' });
  });
  return out;
}

/** Whether the rail is shown for `count` questions. */
export function railShown(count: number): boolean {
  return count >= RAIL_MIN_QUESTIONS;
}

/**
 * The position (in `questions`) of the question in view: the last one at or before the
 * first message in view, so its answer being read keeps its dot filled. Before the first
 * question, the first. -1 when there are none.
 */
export function currentQuestion(questions: readonly RailQuestion[], firstVisibleIndex: number): number {
  if (questions.length === 0) return -1;
  let at = 0;
  for (let i = 0; i < questions.length; i++) {
    if (questions[i].index <= firstVisibleIndex) at = i;
    else break;
  }
  return at;
}

/**
 * The rail's scroll that keeps the dot at `offset` (`size` tall) in a list `viewport`
 * tall now scrolled to `scrollTop`: unchanged when the dot is already in view.
 */
export function keepInView(scrollTop: number, viewport: number, offset: number, size: number): number {
  if (offset < scrollTop) return offset;
  if (offset + size > scrollTop + viewport) return offset + size - viewport;
  return scrollTop;
}
