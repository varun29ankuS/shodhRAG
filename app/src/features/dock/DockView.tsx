/**
 * The floating chat over the embedded Artifacts preview: the `/dock-view` page of the `dock`
 * webview (`dock_layer.rs`). It runs no chat of its own. It shows main's snapshot
 * (`dock-snapshot`) with the same panel and transcript as the in-page chat (`DockPanel`), and
 * sends every action to main through `dock_layer_action`, which main checks before acting. Its
 * hosts for the transcript (the answer engine's setup, code review, web sources and commands)
 * show main's state and send their buttons to main the same way. It never touches a device:
 * the microphone button asks main, which dictates into the shared draft.
 */
import React, { createRef, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { ThemeProvider } from '../../contexts/ThemeContext';
import { DockPanel, PANEL_HEIGHT, PANEL_WIDTH, blockedReasonOf } from '../../components/shell/DockPanel';
import type { DockModel } from '../../components/shell/DockPanel';
import type { AgentComposerHandle } from '../agent/AgentComposer';
import { ModelTrigger } from '../modelPicker/ModelTrigger';
import { TranscriptEffectsContext } from '../agent/transcriptEffects';
import type { TranscriptEffects } from '../agent/transcriptEffects';
import { RuntimeSetupContext } from '../setup/runtimeSetupHost';
import type { RuntimeSetupValue } from '../setup/runtimeSetupHost';
import { CodeReviewContext } from '../code/codeReviewHost';
import type { CodeReviewValue } from '../code/codeReviewHost';
import { matchesShortcut } from '../../lib/shortcuts';
import { parseDockSnapshot } from '../../lib/dockProtocol';
import type { DockAction, DockSnapshot } from '../../lib/dockProtocol';

/** main → dock (`dock_layer::DOCK_SNAPSHOT_EVENT`). */
export const DOCK_SNAPSHOT_EVENT = 'dock-snapshot';

function send(action: DockAction): void {
  invoke('dock_layer_action', { action }).catch(error => console.error('The floating chat could not reach Shodh:', error));
}

function reportSize(width: number, height: number): void {
  invoke('dock_layer_size', { width, height }).catch(error => console.error('The floating chat could not report its size:', error));
}

/**
 * The answer engine's setup as main has it. Install resolves once main's install ends (the
 * snapshot shows it no longer installing), with whether it is installed.
 */
function useRemoteRuntime(snapshot: DockSnapshot | null): RuntimeSetupValue | null {
  const pending = useRef<Array<{ since: number; sawInstalling: boolean; resolve: (installed: boolean) => void }>>([]);
  const runtime = snapshot?.runtime ?? null;
  useEffect(() => {
    if (!runtime) return;
    pending.current = pending.current.filter(request => {
      if (runtime.installing) {
        request.sawInstalling = true;
        return true;
      }
      const finished = request.sawInstalling || (runtime.installedAt ?? 0) >= request.since || runtime.installError !== null;
      if (finished) request.resolve(runtime.installed === true);
      return !finished;
    });
  }, [runtime]);
  return useMemo(() => {
    if (!runtime) return null;
    return {
      ...runtime,
      install: () => new Promise<boolean>(resolve => {
        pending.current.push({ since: Date.now(), sawInstalling: false, resolve });
        send({ kind: 'runtime', op: 'install' });
      }),
      refresh: async () => send({ kind: 'runtime', op: 'refresh' }),
      dismissInstalled: () => send({ kind: 'runtime', op: 'dismissInstalled' }),
      setInstalled: (installed: boolean) => send({ kind: 'runtimeInstalled', installed }),
    };
  }, [runtime]);
}

/**
 * Code review from the floating chat: "Review this" on a code block sends the snippet to main,
 * whose review pane opens on it (and hides this layer while it shows). The pane itself, its
 * checks and the review shortcut live in main, so the rest of the value is main's empty state.
 */
const REMOTE_REVIEW: CodeReviewValue = {
  open: subject => {
    if (subject.kind === 'file') send({ kind: 'review', code: subject.text, language: subject.language });
  },
  close: () => undefined,
  subject: null,
  review: null,
  setReview: () => undefined,
  design: null,
  openDesign: () => undefined,
  setSelection: () => undefined,
  runs: {},
  awaitCheck: () => undefined,
  register: () => () => undefined,
  runRequest: 0,
  paneRef: createRef<HTMLElement>(),
};

/** Web sources and commands, sent to main. */
const REMOTE_EFFECTS: TranscriptEffects = {
  openUrl: async url => send({ kind: 'openUrl', url }),
  stopCommand: async commandId => {
    send({ kind: 'stopCommand', id: commandId });
    return true;
  },
};

export default function DockView() {
  const [snapshot, setSnapshot] = useState<DockSnapshot | null>(null);
  const [listening, setListening] = useState(false);
  const composerRef = useRef<AgentComposerHandle>(null);
  const focusOnOpen = useRef(false);
  const runtime = useRemoteRuntime(snapshot);

  // The page is see-through around the chat's rounded box.
  useEffect(() => {
    document.documentElement.style.background = 'transparent';
    document.body.style.background = 'transparent';
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    listen<unknown>(DOCK_SNAPSHOT_EVENT, event => {
      const next = parseDockSnapshot(event.payload);
      if (next) setSnapshot(next);
    })
      .then(fn => {
        if (disposed) fn();
        else {
          unlisten = fn;
          setListening(true);
        }
      })
      .catch(error => console.error('The floating chat could not listen for the conversation:', error));
    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, []);

  const onAction = useCallback((action: DockAction) => {
    if (action.kind === 'open' && action.open) focusOnOpen.current = true;
    send(action);
  }, []);

  const open = snapshot?.open ?? false;

  // The box this webview is sized to: the open panel, or the pill as it measures. Reported
  // once listening (Rust then sends the latest snapshot), and on every change.
  useEffect(() => {
    if (!listening) return;
    if (open || !snapshot) {
      reportSize(PANEL_WIDTH, open ? PANEL_HEIGHT : 40);
      return;
    }
    const box = document.querySelector<HTMLElement>('[data-dock-box]');
    if (!box) return;
    const observer = new ResizeObserver(() => reportSize(Math.ceil(box.offsetWidth), Math.ceil(box.offsetHeight)));
    observer.observe(box);
    reportSize(Math.ceil(box.offsetWidth), Math.ceil(box.offsetHeight));
    return () => observer.disconnect();
  }, [listening, open, snapshot === null]);

  useEffect(() => {
    if (open && focusOnOpen.current) composerRef.current?.focus();
    focusOnOpen.current = false;
  }, [open]);

  // Ctrl+J opens or minimises the chat here too.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (!matchesShortcut(e, 'toggleDock')) return;
      e.preventDefault();
      onAction({ kind: 'open', open: !open });
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [open, onAction]);

  if (!snapshot || !runtime) return null;

  const model: DockModel = {
    snapshot,
    onAction,
    blockedReason: blockedReasonOf(snapshot, false),
    // The model menu needs main's model list: main shows Ask with it open.
    modelControl: (
      <ModelTrigger
        label={snapshot.model.label}
        ready={snapshot.model.ready}
        onClick={() => onAction({ kind: 'pickModel' })}
        aria-label={`Model: ${snapshot.model.label}. Change model in Ask`}
        title={`${snapshot.model.label} · change in Ask`}
      />
    ),
  };

  return (
    <ThemeProvider forced={snapshot.theme} key={snapshot.theme}>
      <RuntimeSetupContext.Provider value={runtime}>
        <CodeReviewContext.Provider value={REMOTE_REVIEW}>
          <TranscriptEffectsContext.Provider value={REMOTE_EFFECTS}>
            <DockPanel model={model} onExpandAsk={() => send({ kind: 'openInAsk' })} fixed={false} composerRef={composerRef} />
          </TranscriptEffectsContext.Provider>
        </CodeReviewContext.Provider>
      </RuntimeSetupContext.Provider>
    </ThemeProvider>
  );
}
