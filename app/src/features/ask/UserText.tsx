import React from 'react';
import { hardBreaks, wantsMarkdown } from '../focus/summary';
import { MessageContentRenderer } from './MessageContentRenderer';

const noCitation = () => undefined;

/**
 * Text the reader wrote (a question, a posted summary). Plain text keeps
 * its line breaks and never interprets casual `*`, `_` or `#`; text with
 * math, fenced blocks or a table renders as Markdown so equations and
 * diagrams display. Raw HTML is never rendered either way.
 */
export function UserText({ text, compact = false, markdown }: { text: string; compact?: boolean; markdown?: boolean }) {
  const rich = markdown ?? wantsMarkdown(text);
  if (!rich) return <span className="whitespace-pre-wrap break-words">{text}</span>;
  return (
    <div className="min-w-0 break-words [&_.katex-display]:overflow-x-auto [&_.katex-display]:overflow-y-hidden">
      <MessageContentRenderer content={hardBreaks(text)} hits={[]} onOpenCitation={noCitation} compact={compact} citations={false} />
    </div>
  );
}
