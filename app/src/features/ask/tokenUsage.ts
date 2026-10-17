/**
 * The tokens a conversation used, summed from the counts each answer
 * reported (agent runs: the transcript's usage; older answers: their
 * metadata). Shown in the Ask view's "More chat actions" menu.
 *
 * Pure module so it is unit-tested with Node (`app/tests/chatTokenUsage.test.ts`).
 */
import { formatTokens } from '../agent/format.ts';

/** What of a message the sum reads (a `ChatMessage` satisfies it). */
export interface UsageMessage {
  role: string;
  metadata?: { inputTokens?: number | null; outputTokens?: number | null } | null;
  run?: unknown;
  transcript?: { usage: { inputTokens: number; outputTokens: number } | null } | null;
}

export interface ChatTokenUsage {
  input: number;
  output: number;
  /** Answers whose counts are in the sum. */
  counted: number;
  /** Every answer in the conversation. */
  answers: number;
}

function tokens(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : null;
}

/**
 * The sum, or null when no answer reported counts. An answer is an
 * assistant message an answer run produced; notices posted as the assistant
 * (OCR, uploads) carry no run, transcript or metadata and are not answers.
 */
export function chatTokenUsage(messages: readonly UsageMessage[]): ChatTokenUsage | null {
  let input = 0;
  let output = 0;
  let counted = 0;
  let answers = 0;
  for (const m of messages) {
    if (m.role !== 'assistant' || !(m.transcript || m.run || m.metadata)) continue;
    answers += 1;
    const usage = m.transcript?.usage;
    const inTokens = tokens(usage ? usage.inputTokens : m.metadata?.inputTokens);
    const outTokens = tokens(usage ? usage.outputTokens : m.metadata?.outputTokens);
    if (inTokens === null && outTokens === null) continue;
    input += inTokens ?? 0;
    output += outTokens ?? 0;
    counted += 1;
  }
  return counted === 0 ? null : { input, output, counted, answers };
}

/** "Tokens used: 14k in · 1.3k out", with "(for 1 of 3 answers)" when some answers reported none. */
export function tokenUsageLine(usage: ChatTokenUsage | null): string | null {
  if (!usage) return null;
  const partial = usage.counted < usage.answers ? ` (for ${usage.counted} of ${usage.answers} answers)` : '';
  return `Tokens used: ${formatTokens(usage.input)} in · ${formatTokens(usage.output)} out${partial}`;
}
