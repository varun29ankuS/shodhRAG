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
  /** Where the passage sits on its pages (PDF points, bottom-left origin), when known. */
  regions?: FocusRegion[] | null;
}

/** A box on a PDF page in points, bottom-left origin (the indexer's layout boxes). */
export interface FocusRegion {
  page: number;
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

/** A rectangle on a PDF page in points from the top-left corner of its view box. */
export interface FocusRect {
  x: number;
  y: number;
  width: number;
  height: number;
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

/** Where a selection inside a document was made. */
export interface FocusDocumentRef {
  sourceFile: string;
  fileName: string;
  /** 1-based page of a paged document (PDF), when known. */
  page: number | null;
}

/** A slider position of an interactive visual (plot or simulation). */
export interface FocusParamValue {
  name: string;
  value: number;
}

/** The object shown in the pop-out. Everything needed to draw it again. */
export type FocusTarget =
  | { kind: 'mermaid'; label: string; source: string }
  | { kind: 'chart'; label: string; source: string }
  /** A ```svg sketch (source as written; sanitized again whenever drawn). */
  | { kind: 'svg'; label: string; source: string }
  /** A ```plot spec with the slider positions when it was opened. */
  | { kind: 'plot'; label: string; source: string; values: FocusParamValue[] }
  /** A ```simulation spec with the slider positions when it was opened. */
  | { kind: 'simulation'; label: string; source: string; values: FocusParamValue[] }
  | { kind: 'equation'; label: string; tex: string }
  | { kind: 'table'; label: string; rows: string[][] }
  | { kind: 'image'; label: string; src: string | null; alt: string }
  | { kind: 'source'; label: string; hit: FocusSourceHit }
  | { kind: 'task'; label: string; task: FocusTaskSnapshot }
  /**
   * A saved snippet (page region): its id, place and text. The image stays in
   * the snippet store and is loaded by id when drawn.
   */
  | { kind: 'snippet'; label: string; snippetId: string; filePath: string; fileName: string; page: number; rect: FocusRect; text: string }
  /**
   * Text the reader selected in an answer or a document, with the paragraph
   * around it. `document` is set when it was selected in a document viewer.
   */
  | {
      kind: 'selection';
      label: string;
      text: string;
      paragraph: string;
      origin: 'answer' | 'document';
      document: FocusDocumentRef | null;
    };

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
  /** Suggested next questions the answer ended with (side answers only). */
  followups?: string[];
  /** This user turn is a summary brought up from a nested discussion. */
  summaryOf?: { threadId: string; label: string };
}

export interface FocusThread {
  id: string;
  anchor: ThreadAnchor;
  /**
   * Thread this one was opened from (drill-down inside a side answer).
   * Absent for a thread opened from the conversation itself.
   */
  parentThreadId?: string;
  /** Answer turn of the parent thread the object was found in. */
  parentTurnId?: string;
  turns: ThreadTurn[];
  createdAt: string;
  updatedAt: string;
}
