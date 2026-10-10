/**
 * What fenced blocks of one answer need from the answer around them, given
 * through context so the renderer's code components keep a stable identity
 * (a new component would remount every visual below it): the symbol
 * meanings of the answer and a renderer for inline text with citation pills.
 */

import { createContext, useContext } from 'react';
import type { ReactNode } from 'react';
import type { SymbolNote } from './symbols';

export interface AnswerBlocks {
  symbols: readonly SymbolNote[];
  /** Text with `[n]` citations as pills (plain text where citations are off). */
  renderInline: (text: string) => ReactNode;
  /**
   * The text is a model's answer (not text the reader wrote, a summary
   * card or a printout): a diagram in it that does not draw may be sent
   * back to the model for one correction.
   */
  modelAnswer: boolean;
  /**
   * The source numbers of the answer, which a diagram may cite; null for
   * text without sources (citations are then refused).
   */
  citations: ReadonlySet<number> | null;
  /** Open source `n` of the answer (as its citation pill does). */
  openCitation: ((n: number, trigger: HTMLElement) => void) | null;
}

const NONE: AnswerBlocks = { symbols: [], renderInline: text => text, modelAnswer: false, citations: null, openCitation: null };

export const AnswerBlocksContext = createContext<AnswerBlocks>(NONE);

export function useAnswerBlocks(): AnswerBlocks {
  return useContext(AnswerBlocksContext);
}
