/**
 * The main window acts on what the floating chat over the embedded preview sends
 * (`lib/dockProtocol`'s `DockAction`, already checked by `parseDockAction`). The chat holds no
 * session: every action runs here, on main's own session, draft, dictation and panes, exactly
 * as the same click in the in-page chat would. Pure, so the Node tests load it.
 */
import type { DockAction, RuntimeOp } from '../../lib/dockProtocol';

/** What main runs the floating chat's actions on. */
export interface DockTarget {
  /** An answer runs in the active conversation now. */
  isStreaming: boolean;
  /** The shared draft now. */
  draft: string;
  /** Send the draft, through the same path as the in-page chat (`submitDockDraft`). */
  send: () => void;
  /** Redirect the running answer with `text`. */
  steer: (text: string) => void;
  cancel: () => void;
  approve: (id: string, allow: boolean, forConversation?: boolean, reviewToken?: string) => void;
  setDraft: (text: string) => void;
  /** Main's dictation into the shared draft (main holds the microphone). */
  dictation: { listening: boolean; toggle: () => void };
  setOpen: (open: boolean) => void;
  openInAsk: () => void;
  close: () => void;
  /** The user typed in the chat: the assistant's moves wait, as for typing in this window. */
  typed: () => void;
  runtime: (op: RuntimeOp) => void;
  setRuntimeInstalled: (installed: boolean) => void;
  /** Open main's code review pane on a code block of the answer. */
  review: (code: string, language: string) => void;
  openUrl: (url: string) => void;
  stopCommand: (commandId: string) => void;
  openSettings: () => void;
}

export function applyDockAction(action: DockAction, target: DockTarget): void {
  switch (action.kind) {
    case 'send':
      target.send();
      return;
    case 'steer': {
      const text = target.draft.trim();
      // As in the in-page chat: only a running answer is steered, and only with words.
      if (!target.isStreaming || !text) return;
      target.setDraft('');
      target.steer(text);
      return;
    }
    case 'cancel':
      target.cancel();
      return;
    case 'approve':
      target.approve(action.id, action.allow, action.forConversation, action.reviewToken);
      return;
    case 'draft':
      target.setDraft(action.text);
      target.typed();
      return;
    case 'dictate':
      if (action.on !== target.dictation.listening) target.dictation.toggle();
      return;
    case 'open':
      target.setOpen(action.open);
      return;
    case 'openInAsk':
      target.openInAsk();
      return;
    case 'close':
      target.close();
      return;
    case 'runtime':
      target.runtime(action.op);
      return;
    case 'runtimeInstalled':
      target.setRuntimeInstalled(action.installed);
      return;
    case 'review':
      target.review(action.code, action.language);
      return;
    case 'openUrl':
      target.openUrl(action.url);
      return;
    case 'stopCommand':
      target.stopCommand(action.id);
      return;
    case 'openSettings':
      target.openSettings();
      return;
  }
}

/** What sending the draft needs from main: the in-page chat and the floating chat share it. */
export interface DraftSubmit {
  conversationId: string | null;
  isStreaming: boolean;
  busyElsewhere: boolean;
  /** A file check of an earlier send is still running. */
  checking: () => boolean;
  setChecking: (on: boolean) => void;
  /** The draft, attachments and file the composer holds now (the stores, read at send). */
  currentDraft: () => string;
  queuedFile: () => { path: string } | null;
  /** Send `text` limited to `files`; false when the session did not take it. */
  send: (text: string, files: readonly string[]) => boolean;
  /** The question was taken: clear the draft and attachments, keep the file scope. */
  sent: (file: { path: string } | null) => void;
  /** Before Shodh is ready, the question waits for setup (true: it is held). */
  hold: (sendNow: () => boolean) => boolean;
  /** Paths of `files` the index no longer has. */
  missing: (files: readonly string[]) => Promise<readonly string[]>;
  reportMissing: (missing: readonly string[]) => void;
  /** The conversation shown now (a switch during the file check cancels the send). */
  shownConversation: () => string | null;
}

/** Send the shared draft, as Ask does: setup may hold it, and a chosen file is checked first. */
export function submitDockDraft(draft: string, ports: DraftSubmit): void {
  if (!draft.trim() || ports.isStreaming || ports.busyElsewhere || ports.checking()) return;
  const conversationId = ports.conversationId;
  const sendComposer = (): boolean => {
    const current = ports.currentDraft().trim();
    if (!current) return true;
    const file = ports.queuedFile();
    if (!ports.send(current, file ? [file.path] : [])) return false;
    ports.sent(file);
    return true;
  };
  if (ports.hold(sendComposer)) return;
  const file = ports.queuedFile();
  if (!file) {
    sendComposer();
    return;
  }
  ports.setChecking(true);
  void ports
    .missing([file.path])
    .then(missing => {
      if (ports.shownConversation() !== conversationId) return;
      if (missing.length === 0) sendComposer();
      else ports.reportMissing(missing);
    })
    .finally(() => ports.setChecking(false));
}
