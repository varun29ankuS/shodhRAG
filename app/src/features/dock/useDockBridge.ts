/**
 * Runs in the main window while the floating chat's layer is up: it publishes what the chat
 * shows to the `dock` webview (`dock_layer_publish`, at most once a frame) and acts on what the
 * chat sends back (`dock-action`, checked by `parseDockAction`) on main's own session, draft,
 * dictation and panes. Dictation runs here, in main, which holds the microphone; the chat only
 * asks for it, and the transcript reaches it through the shared draft in the next snapshot.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useChatSession } from '../ask/ChatSessionContext';
import { useDraft } from '../ask/useDraft';
import { useVoiceInput } from '../ask/useVoiceInput';
import type { OptionSource } from '../ask/sendOptions';
import type { ChatMessage } from '../ask/types';
import { useRuntimeSetup, runtimeSetupState } from '../setup/runtimeSetupHost';
import { useCodeReview } from '../code/codeReviewHost';
import { openExternal } from '../agent/WebSources';
import { agentApi, toAgentError } from '../agent/agentApi';
import { useTheme } from '../../contexts/ThemeContext';
import { typingActivity } from '../../lib/navigationGate';
import { matchesShortcut } from '../../lib/shortcuts';
import { notify } from '../../lib/notify';
import { parseDockAction, snapshotOf } from '../../lib/dockProtocol';
import { applyDockAction } from './dockActions';
import type { DockTarget } from './dockActions';
import { useDraftSubmit } from './useDraftSubmit';
import { useConversationModes } from '../ask/useConversationModes';
import { useDockModel } from '../modelPicker/useModelLabel';
import { requestModelPicker } from '../modelPicker/navigation';
import { useSendKey } from '../../hooks/useSendKey';

/** dock → main (`dock_layer::DOCK_ACTION_EVENT`). */
export const DOCK_ACTION_EVENT = 'dock-action';

/** Grows with every new message, step and text segment: the unread measure. */
function activitySize(messages: readonly ChatMessage[]): number {
  let size = messages.length;
  for (const m of messages) size += m.transcript?.blocks.length ?? 0;
  return size;
}

interface DockBridgeOptions {
  /** The layer is up (the embedded preview shows). */
  active: boolean;
  /** The Library's sources: replies search the included ones, as in Ask. */
  sources: readonly OptionSource[];
  /** Return to the Ask view (full conversation). */
  onExpand: () => void;
}

export function useDockBridge({ active, sources, onExpand }: DockBridgeOptions): void {
  const session = useChatSession();
  const { messages, isStreaming, navigation, steer, cancel, approve, setRuntimeInstalled, streamingConversationId, sideRun } = session;
  const conversationId = session.activeConversationId;
  const [draft, setDraft] = useDraft(conversationId);
  const draftRef = useRef(draft);
  draftRef.current = draft;
  const getDraft = useCallback(() => draftRef.current, []);
  // Main's microphone, into main's draft; the chat shows it through the snapshot.
  const voice = useVoiceInput(getDraft, setDraft);
  const runtime = useRuntimeSetup();
  const review = useCodeReview();
  const { theme } = useTheme();
  const { submit } = useDraftSubmit(sources);
  // The composer's mode menu and model control, as Ask shows them.
  const modes = useConversationModes();
  const { model: modelLabel, choose: chooseModel } = useDockModel(active);
  const { sendKey } = useSendKey();
  // The layer shows while the assistant builds the page the user is watching: open at first.
  const [open, setOpen] = useState(true);
  const [unreadBase, setUnreadBase] = useState<number | null>(null);

  // The assistant opened a view for the user to watch: the chat opens with it.
  const lastNavSeq = useRef(navigation?.seq ?? 0);
  useEffect(() => {
    if (!navigation || navigation.seq === lastNavSeq.current) return;
    lastNavSeq.current = navigation.seq;
    if (navigation.openDock) setOpen(true);
  }, [navigation]);

  const size = activitySize(messages);
  useEffect(() => {
    if (!open) setUnreadBase(base => (base === null ? size : base));
    else setUnreadBase(null);
  }, [open, size]);
  const unread = unreadBase === null ? 0 : Math.max(0, size - unreadBase);

  const snapshot = useMemo(() => snapshotOf({
    conversationId,
    title: session.activeConversation?.title ?? '',
    messages,
    isStreaming,
    streamingConversationId,
    sideRun,
    readOnlyConnector: session.activeConversation?.source?.connector ?? null,
    draft,
    dictating: voice.listening,
    voiceSupported: voice.supported,
    open,
    unread,
    theme,
    runtime: runtimeSetupState(runtime),
    work: modes.work,
    permission: modes.permission,
    codeAvailable: modes.codeAvailable,
    model: modelLabel,
    sendKey,
  }), [conversationId, session.activeConversation?.title, session.activeConversation?.source?.connector, messages, isStreaming, streamingConversationId, sideRun, draft, voice.listening, voice.supported, open, unread, theme, runtime, modes.work, modes.permission, modes.codeAvailable, modelLabel, sendKey]);

  // At most one publish a frame, the latest snapshot.
  useEffect(() => {
    if (!active) return;
    const frame = window.requestAnimationFrame(() => {
      invoke('dock_layer_publish', { snapshot }).catch(error => console.error('The floating chat could not be updated:', error));
    });
    return () => window.cancelAnimationFrame(frame);
  }, [active, snapshot]);

  const target: DockTarget = {
    isStreaming,
    draft,
    send: () => submit(draftRef.current),
    steer,
    cancel,
    approve,
    setDraft,
    dictation: { listening: voice.listening, toggle: voice.toggle },
    setOpen,
    openInAsk: onExpand,
    // Closing the chat over the preview leaves its pill: the layer stays while the page shows.
    close: () => setOpen(false),
    typed: () => typingActivity.noteRemote(Date.now()),
    runtime: op => {
      if (op === 'install') void runtime.install();
      else if (op === 'refresh') void runtime.refresh();
      else runtime.dismissInstalled();
    },
    setRuntimeInstalled,
    review: (code, language) => review?.open({
      kind: 'file',
      title: `Code from the answer (${language})`,
      path: 'snippet',
      text: code,
      language,
      firstLine: 1,
      workspaceId: null,
      selection: null,
    }),
    openUrl: url => void openExternal(url),
    stopCommand: commandId => {
      agentApi.stopCommand(commandId).catch(error => notify.error('The command could not be stopped', { description: toAgentError(error).message }));
    },
    openSettings: () => {
      window.dispatchEvent(new CustomEvent('switchTab', { detail: 'settings' }));
    },
    setWork: modes.pickWork,
    setPermission: modes.changeAgentMode,
    pickModel: requestModelPicker,
    setModel: id => {
      void chooseModel(id).then(result => {
        if (result.kind === 'changed') {
          notify.success(isStreaming ? `${result.name} will answer your next question` : `${result.name} will answer from now on`);
        } else if (result.kind === 'refused') {
          notify.error('The model was not changed', { description: result.error.message });
        } else if (result.kind === 'unknown') {
          notify.error('The model was not changed', { description: 'That model is no longer offered. Open the menu again.' });
        }
      });
    },
  };
  const targetRef = useRef(target);
  targetRef.current = target;

  useEffect(() => {
    if (!active) return;
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen<unknown>(DOCK_ACTION_EVENT, event => {
      const action = parseDockAction(event.payload);
      if (action) applyDockAction(action, targetRef.current);
      else console.warn('The floating chat sent something that is not an action; ignored.');
    })
      .then(fn => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(error => console.error('The floating chat could not be heard:', error));
    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, [active]);

  // Ctrl+J in the main window opens or minimises the chat over the preview too.
  useEffect(() => {
    if (!active) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (!matchesShortcut(e, 'toggleDock')) return;
      e.preventDefault();
      setOpen(value => !value);
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [active]);
}

/** Mounted in place of the in-page chat while the layer is up. */
export function DockBridge(props: DockBridgeOptions): null {
  useDockBridge(props);
  return null;
}
