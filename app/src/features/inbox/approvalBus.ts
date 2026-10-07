/**
 * An approval answered outside its chat (in the Inbox): the chat that shows the
 * step marks it decided, so it does not keep offering Approve / Deny for a step
 * the assistant already moved past.
 */

export interface ApprovalAnswer {
  sessionId: string;
  stepId: string;
  approved: boolean;
}

const EVENT = 'shodh:approval-answered';

export function announceApproval(answer: ApprovalAnswer): void {
  window.dispatchEvent(new CustomEvent<ApprovalAnswer>(EVENT, { detail: answer }));
}

/** Listen for answers; returns the unsubscribe function. */
export function onApprovalAnswered(handler: (answer: ApprovalAnswer) => void): () => void {
  const listener = (event: Event) => handler((event as CustomEvent<ApprovalAnswer>).detail);
  window.addEventListener(EVENT, listener);
  return () => window.removeEventListener(EVENT, listener);
}
