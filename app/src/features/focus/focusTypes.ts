/**
 * Shapes of the focus pop-out: what is focused (the target), where a side
 * thread is anchored, and the persisted side-thread record.
 *
 * Pure module (types only) so the logic around it is unit-tested with Node.
 */

export interface FocusPageSpan {
  start: number;
  end: number;
}

/** A cited source as the document viewers need it (subset of `SearchHit`). */
export interface FocusSourceHit {
  number: number;
  sourceFile: string;
  fileName: string;
  title: string;
  text: string;
  snippet: string;
  score: number;
  page: FocusPageSpan | null;
  lineRange: [number, number] | null;
  url: string | null;
}

/** A task as the side thread describes it to the agent. */
export interface FocusTaskSnapshot {
  id: string;
  title: string;
  status: string;
  priority: string;
  dueDate: string | null;
  notes: string;
  tags: string[];
  subtasks: { title: string; completed: boolean }[];
  project: string | null;
}

/** The object shown in the pop-out. Everything needed to draw it again. */
export type FocusTarget =
  | { kind: 'mermaid'; label: string; source: string }
  | { kind: 'chart'; label: string; source: string }
  | { kind: 'equation'; label: string; tex: string }
  | { kind: 'table'; label: string; rows: string[][] }
  | { kind: 'image'; label: string; src: string | null; alt: string }
  | { kind: 'source'; label: string; hit: FocusSourceHit }
  | { kind: 'task'; label: string; task: FocusTaskSnapshot };

export type FocusKind = FocusTarget['kind'];

/** Live details of the pop-out at the moment a question is sent. */
export interface FocusExtras {
  /** Text the reader selected inside the focused object. */
  selection?: string | null;
  /** Page of a document the reader is looking at. */
  page?: number | null;
}

/** Where a side thread hangs in the main conversation. */
export interface ThreadAnchor {
  conversationId: string;
  /** Message the focused object came from; null for objects outside the conversation (tasks). */
  parentMessageId: string | null;
  target: FocusTarget;
}

export interface ThreadTurn {
  id: string;
  role: 'user' | 'assistant';
  /** The question as typed (never the attached context block), or the answer text. */
  content: string;
  timestamp: string;
  /** Selection that was attached to this question. */
  selection?: string;
  /** Page of the document this question was asked on. */
  page?: number;
  /** Persisted agent transcript of an answer (see features/agent/reducer). */
  transcript?: Record<string, unknown>;
}

export interface FocusThread {
  id: string;
  anchor: ThreadAnchor;
  turns: ThreadTurn[];
  createdAt: string;
  updatedAt: string;
}
