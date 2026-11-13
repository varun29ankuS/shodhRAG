/**
 * An answer: its structure (paragraphs, headings, lists, code, tables) in
 * reading type, its citations as chips that open the cited page, and, once
 * it is finished, a line drawn under it and the pages it cites fanning in.
 *
 * While it streams, each new word fades in where it lands; words never move
 * once shown, and nothing waits for an animation.
 */
import { Fragment, type MouseEvent, type ReactNode } from 'react';
import { motion } from 'framer-motion';
import { Globe } from 'lucide-react';
import { answerText, citedNumbers, isLive, type AnswerState, type Passage } from '../lib/answer';
import { parseAnswer, type Block, type Inline } from '../lib/markdown';
import { fanIn, popIn, sweep } from '../lib/motion';
import type { Origin } from '../lib/nav';
import { PageThumb } from './PageThumb';

export interface CiteHandler {
  (passage: Passage, origin: Origin | null): void;
}

function originOf(event: MouseEvent<HTMLElement>): Origin {
  const rect = event.currentTarget.getBoundingClientRect();
  return { x: rect.left, y: rect.top, width: rect.width, height: rect.height };
}

/** Words of a text, each with the space after it, so a new word can fade in alone. */
function words(text: string): string[] {
  return text.match(/\S+\s*|\s+/g) ?? [];
}

function Inlines({ parts, live, passages, onCite }: { parts: Inline[]; live: boolean; passages: Passage[]; onCite: CiteHandler }) {
  let index = 0;
  return (
    <>
      {parts.map((part, i) => {
        if (part.kind === 'cite') {
          const passage = passages.find(p => p.n === part.n) ?? null;
          return (
            <motion.button
              key={`c${i}`}
              type="button"
              className="chip"
              aria-label={passage ? `Source ${part.n}: ${passage.title}` : `Source ${part.n}`}
              disabled={!passage}
              onClick={event => passage && onCite(passage, originOf(event))}
              variants={popIn}
              initial={live ? 'hidden' : false}
              animate="shown"
            >
              {part.n}
            </motion.button>
          );
        }
        const content: ReactNode = live
          ? words(part.text).map(word => (
              <span key={index++} className="word">
                {word}
              </span>
            ))
          : part.text;
        if (part.code) return <code key={`t${i}`} className="inline-code">{content}</code>;
        if (part.strong && part.em)
          return (
            <strong key={`t${i}`}>
              <em>{content}</em>
            </strong>
          );
        if (part.strong) return <strong key={`t${i}`}>{content}</strong>;
        if (part.em) return <em key={`t${i}`}>{content}</em>;
        return <Fragment key={`t${i}`}>{content}</Fragment>;
      })}
    </>
  );
}

function BlockView({ block, live, passages, onCite }: { block: Block; live: boolean; passages: Passage[]; onCite: CiteHandler }) {
  switch (block.kind) {
    case 'p':
      return (
        <p className="block-p">
          <Inlines parts={block.inlines} live={live} passages={passages} onCite={onCite} />
        </p>
      );
    case 'h':
      return (
        <h2 className="block-h">
          <Inlines parts={block.inlines} live={false} passages={passages} onCite={onCite} />
        </h2>
      );
    case 'li':
      return (
        <div className="block-li">
          {block.marker ? <span className="number">{block.marker}</span> : <span className="bullet" aria-hidden="true" />}
          <p>
            <Inlines parts={block.inlines} live={live} passages={passages} onCite={onCite} />
          </p>
        </div>
      );
    case 'quote':
      return (
        <blockquote className="block-quote">
          <Inlines parts={block.inlines} live={live} passages={passages} onCite={onCite} />
        </blockquote>
      );
    case 'code':
      return <pre className="block-code">{block.text}</pre>;
    case 'table':
      return (
        <div className="block-table">
          <table>
            <tbody>
              {block.rows.map((row, r) => (
                <tr key={r}>
                  {row.map((cell, c) => (
                    <td key={c}>
                      <Inlines parts={cell} live={false} passages={passages} onCite={onCite} />
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
  }
}

/** The answer's blocks. */
export function AnswerBlocks({ answer, onCite }: { answer: AnswerState; onCite: CiteHandler }) {
  const live = isLive(answer);
  const blocks = parseAnswer(answerText(answer));
  if (blocks.length === 0) return null;
  return (
    <div className="blocks" aria-live={live ? 'polite' : undefined} aria-busy={live}>
      {blocks.map((block, i) => (
        <BlockView key={i} block={block} live={live} passages={answer.passages} onCite={onCite} />
      ))}
    </div>
  );
}

/** Where a source is, in words: "Page 4", a heading, or the web site. */
export function whereOf(passage: Passage): string {
  if (passage.web) {
    try {
      return new URL(passage.path).hostname.replace(/^www\./, '');
    } catch {
      return 'Web page';
    }
  }
  if (passage.page !== null) return `Page ${passage.page}`;
  return passage.heading ?? 'In your library';
}

/** Once finished: the line under the answer, and the sources it cites, fanning in. */
export function AnswerSources({ answer, onCite }: { answer: AnswerState; onCite: CiteHandler }) {
  if (isLive(answer)) return null;
  const cited = citedNumbers(answerText(answer))
    .map(n => answer.passages.find(p => p.n === n))
    .filter((p): p is Passage => p !== undefined);
  const library = cited.filter(p => !p.web);
  const web = cited.filter(p => p.web);
  return (
    <>
      <motion.span className="sweep" aria-hidden="true" variants={sweep} initial="hidden" animate="shown" />
      {library.length > 0 && (
        <>
          <div className="label">From your library</div>
          <div className="sources">
            {library.map((passage, i) => (
              <motion.button
                key={passage.n}
                type="button"
                className="source"
                aria-label={`Open ${passage.title}, ${whereOf(passage)}`}
                onClick={event => onCite(passage, originOf(event))}
                variants={fanIn}
                custom={i}
                initial="hidden"
                animate="shown"
              >
                <PageThumb path={passage.path} page={passage.page} badge={passage.n} />
                <span className="row-text">
                  <span className="source-title">{passage.title}</span>
                  <span className="source-where">{whereOf(passage)}</span>
                </span>
              </motion.button>
            ))}
          </div>
        </>
      )}
      {web.length > 0 && (
        <>
          <div className="label">From the web</div>
          <div className="rows">
            {web.map(passage => (
              <button key={passage.n} type="button" className="row" onClick={event => onCite(passage, originOf(event))}>
                <span className="row-icon">
                  <Globe aria-hidden="true" className="icon-small" />
                </span>
                <span className="row-text">
                  <span className="row-title">{passage.title}</span>
                  <span className="row-sub">{`[${passage.n}] ${whereOf(passage)}`}</span>
                </span>
              </button>
            ))}
          </div>
        </>
      )}
    </>
  );
}
