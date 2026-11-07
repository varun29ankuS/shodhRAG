/**
 * Words for Code mode's tokens-saved counter (per conversation, never per
 * answer) and its running total in Activity. Every number is an estimate, so
 * every number carries "≈". Pure functions (tested in tests/codeIntel.test.ts).
 */

export interface SavingsCounts {
  savedTokens: number;
  originalTokens: number;
  sentTokens: number;
  commands: number;
  shortenedCommands: number;
}

/** 41200 → "≈41,200". */
export function approxTokens(n: number): string {
  const whole = Math.max(0, Math.round(n));
  return `≈${whole.toLocaleString('en-US')}`;
}

function plural(n: number, word: string): string {
  return `${n.toLocaleString('en-US')} ${word}${n === 1 ? '' : 's'}`;
}

/** The status line's chip: "≈41,200 tokens saved", or whether shortening is on before a command ran. */
export function chipLabel(view: SavingsCounts & { compressionOn: boolean }): string {
  if (!view.compressionOn) return 'Output shortening off';
  if (view.commands === 0) return 'Output shortening on';
  return `${approxTokens(view.savedTokens)} tokens saved`;
}

/**
 * Whether the status line shows the chip: in every Code conversation, from before its first command, so the
 * switch can be reached before anything is shortened. Not before its savings are read.
 */
export function showsChip(view: (SavingsCounts & { compressionOn: boolean }) | null): boolean {
  return view !== null;
}

/** "Saved ≈41,200 tokens in this conversation" (one number per conversation, kept with it). */
export function sessionSentence(view: SavingsCounts): string {
  return `Saved ${approxTokens(view.savedTokens)} tokens in this conversation`;
}

/** "The model read ≈8,800 tokens of command output instead of ≈50,000, over 12 commands." */
export function sessionDetail(view: SavingsCounts): string {
  if (view.commands === 0) return 'No command has run in this conversation yet.';
  return `The model read ${approxTokens(view.sentTokens)} tokens of command output instead of ${approxTokens(
    view.originalTokens,
  )}, over ${plural(view.commands, 'command')}.`;
}

/** Share of the output the model did not need, as a whole percent ("82%"), or null with nothing counted. */
export function savedShare(view: SavingsCounts): string | null {
  if (view.originalTokens <= 0) return null;
  return `${Math.round((view.savedTokens / view.originalTokens) * 100)}%`;
}

/** Activity's card detail: "82% of command output · 40 commands in 3 conversations". */
export function totalDetail(total: SavingsCounts & { conversations: number }): string {
  if (total.commands === 0) return 'Counted from the first Code mode command';
  const share = savedShare(total);
  const parts = [
    share ? `${share} of command output` : null,
    `${plural(total.commands, 'command')} in ${plural(total.conversations, 'conversation')}`,
  ].filter((p): p is string => p !== null);
  return parts.join(' · ');
}
