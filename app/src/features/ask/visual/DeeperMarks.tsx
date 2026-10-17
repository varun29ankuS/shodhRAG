import { Sparkles } from 'lucide-react';
import { cn } from '../../../lib/utils';
import { useOpenFocus } from '../../focus/FocusFrame';
import { selectionTarget } from '../../focus/targets';
import { useAnswerBlocks } from './answerContext';
import { blockMarks } from './marks';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

/**
 * A "go deeper" mark in prose: the shown text, dotted underline, opening a
 * nested window that asks the prepared question. Plain text where nothing
 * can be opened (no answer around, a printout).
 */
export function MarkText({ shown, question, context }: { shown: string; question: string; context: string }) {
  const openFocus = useOpenFocus();
  if (!openFocus) return <>{shown}</>;
  return (
    <button
      type="button"
      title={question}
      aria-label={`${shown}. Go deeper: ${question}`}
      className={cn(
        'inline p-0 m-0 bg-transparent border-0 font-[inherit] text-[inherit] text-left cursor-pointer',
        'underline decoration-dotted decoration-shodh-accent underline-offset-[3px] hover:decoration-solid',
        FOCUS_RING,
      )}
      onClick={e => {
        const target = selectionTarget({ text: shown, context, origin: 'answer' });
        if (target) openFocus(target, e.currentTarget, { question });
      }}
    >
      {shown}
    </button>
  );
}

/**
 * The "go deeper" marks of a chart, plot, simulation, derivation or symbol
 * list, as chips under it (diagrams draw theirs on the node). Only as many
 * as the answer's budget gives this block.
 */
export function DeeperChips({ lang, source }: { lang: string; source: string }) {
  const { markAllowance } = useAnswerBlocks();
  const openFocus = useOpenFocus();
  const allowed = markAllowance(source);
  if (!openFocus || allowed === 0) return null;
  const marks = blockMarks(lang, source.trim()).slice(0, allowed);
  if (marks.length === 0) return null;
  return (
    <div className="-mt-2 mb-4 flex flex-wrap items-center gap-1.5" role="group" aria-label="Go deeper">
      <span className="text-[11.5px] text-shodh-text-muted" data-answer-ui="">Go deeper:</span>
      {marks.map(mark => (
        <button
          key={`${mark.label}\n${mark.question}`}
          type="button"
          title={mark.question}
          className={cn(
            'inline-flex items-center gap-1 h-6 px-2 rounded-full border border-shodh-border bg-shodh-surface text-[11.5px] text-shodh-text-secondary',
            'hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
            FOCUS_RING,
          )}
          onClick={e => {
            const target = selectionTarget({ text: mark.label, context: mark.question, origin: 'answer' });
            if (target) openFocus(target, e.currentTarget, { question: mark.question });
          }}
        >
          <Sparkles className="w-3 h-3 text-shodh-accent" aria-hidden="true" />
          {/* The label is the block's own words; the suggested question is UI and never quoted. */}
          <span className="font-medium">{mark.label}</span>
          <span className="text-shodh-text-muted truncate max-w-[260px]" data-answer-ui="">{mark.question}</span>
        </button>
      ))}
    </div>
  );
}
