import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useNavigationTarget } from '../agent/useNavigationTarget';
import { useAgentSession } from '../agent/useAgentSession';
import type { AgentEventEnvelope } from '../agent/events';
import { matchesShortcut } from '../../lib/shortcuts';
import { notify } from '../../lib/notify';
import { codeApi, designApi } from './api';
import { languageOf } from './diffModel';
import type { LineSelection } from './diffModel';
import { linesAtOffsets, piecesAsChanges, reviewedSelection, withOutput } from './reviewModel';
import type { CheckRun, ReviewSubject, StoredReview } from './reviewModel';
import { opensPaneForAssistant } from './designModel';
import type { StoredDesignCheck } from './designModel';
import { CodeReviewContext, isTextEntry } from './codeReviewHost';
import type { CodeReviewValue, ReviewSource } from './codeReviewHost';

// The value's type, context, hook and helpers live in ./codeReviewHost; importers keep
// finding them here.
export * from './codeReviewHost';

export function CodeReviewProvider({ children }: { children: React.ReactNode }) {
  const [subject, setSubject] = useState<ReviewSubject | null>(null);
  const [review, setReview] = useState<StoredReview | null>(null);
  const [design, setDesign] = useState<StoredDesignCheck | null>(null);
  const [runs, setRuns] = useState<Record<string, CheckRun>>({});
  const [runRequest, setRunRequest] = useState(0);
  const sources = useRef(new Set<ReviewSource>());
  const paneRef = useRef<HTMLElement | null>(null);
  // Checks sent and not yet started: their command, by key.
  const awaited = useRef(new Map<string, string>());
  // Started checks: key by Shodh's command id.
  const byCommand = useRef(new Map<string, string>());

  const open = useCallback((next: ReviewSubject) => {
    setSubject(next);
    setReview(null);
    setDesign(null);
  }, []);
  const close = useCallback(() => {
    setSubject(null);
    setReview(null);
    setDesign(null);
  }, []);
  const openDesign = useCallback((check: StoredDesignCheck, next?: ReviewSubject | null) => {
    setSubject(next ?? null);
    setReview(null);
    setDesign(check);
  }, []);
  const setSelection = useCallback((selection: LineSelection | null) => {
    setSubject(current => (current && current.kind !== 'folder' ? { ...current, selection } : current));
  }, []);
  const awaitCheck = useCallback((key: string, command: string) => {
    awaited.current.set(key, command.trim());
    setRuns(r => ({ ...r, [key]: { command, commandId: null, state: 'waiting', exitCode: null, output: '', note: null } }));
  }, []);
  const register = useCallback((source: ReviewSource) => {
    sources.current.add(source);
    return () => {
      sources.current.delete(source);
    };
  }, []);

  // A review the assistant ran (`review_code`): the pane opens on it, its lines selected.
  useNavigationTarget('review', target => {
    codeApi
      .get(target.reviewId)
      .then(found => {
        const changes = piecesAsChanges(found.request.pieces);
        if (found.request.folder && changes.length === 0 && found.workspaceId) {
          setSubject({ kind: 'folder', title: `Architecture of ${found.request.folder}`, folder: found.request.folder, workspaceId: found.workspaceId });
        } else {
          setSubject({
            kind: 'diff',
            title: 'Reviewed by the assistant',
            changes,
            workspaceId: found.workspaceId,
            selection: reviewedSelection(found.request),
          });
        }
        setReview(found);
      })
      .catch(e => notify.error('The review could not be opened', { description: String(e) }));
  });

  // A design check the assistant ran (`design_check`): the pane opens on a project's findings.
  // A creation's check stays its page's badge (Artifacts), so it never covers the page.
  useNavigationTarget('design_check', target => {
    designApi
      .get(target.checkId)
      .then(found => {
        if (opensPaneForAssistant(found)) openDesign(found);
      })
      .catch(e => notify.error('The design check could not be opened', { description: String(e) }));
  });

  // The assistant pointed at code (`open_code`): a file at lines, or the conversation's changes.
  useNavigationTarget('code', target => {
    const selection: LineSelection | null = target.path && target.line
      ? { path: target.path, side: 'new', from: target.line, to: target.endLine ?? target.line }
      : null;
    if (target.view === 'file' && target.path) {
      const path = target.path;
      codeApi
        .file(target.workspaceId, path)
        .then(file =>
          open({ kind: 'file', title: path, path, text: file.text, language: file.language === 'text' ? languageOf(path) : file.language, firstLine: 1, workspaceId: target.workspaceId, selection }),
        )
        .catch(e => notify.error('The file could not be opened', { description: String(e) }));
      return;
    }
    codeApi
      .diff(target.workspaceId, target.conversationId, target.path)
      .then(changes => open({ kind: 'diff', title: target.path ? `Changes of ${target.path}` : 'Code changes', changes, workspaceId: target.workspaceId, selection }))
      .catch(e => notify.error('The changes could not be read', { description: String(e) }));
  });

  // Commands the conversation runs: a check the pane sent is followed by its exact command.
  const onAgentEvent = useCallback((envelope: AgentEventEnvelope) => {
    const event = envelope.event;
    if (event.type === 'command_started') {
      for (const [key, command] of awaited.current) {
        if (command === event.command.trim()) {
          awaited.current.delete(key);
          byCommand.current.set(event.commandId, key);
          setRuns(r => (r[key] ? { ...r, [key]: { ...r[key], commandId: event.commandId, state: 'running' } } : r));
          break;
        }
      }
    } else if (event.type === 'command_output') {
      const key = byCommand.current.get(event.commandId);
      if (key) setRuns(r => (r[key] ? { ...r, [key]: withOutput(r[key], event.text) } : r));
    } else if (event.type === 'command_finished') {
      const key = byCommand.current.get(event.commandId);
      if (!key) return;
      byCommand.current.delete(event.commandId);
      setRuns(r => (r[key] ? { ...r, [key]: { ...r[key], state: event.state, exitCode: event.exitCode, note: event.note } } : r));
    }
  }, []);
  useAgentSession(onAgentEvent);

  // "Review this" from the keyboard: inside the open pane, run it; on a reviewable code
  // view the focus or a selection is in, open the pane on it. Typing elsewhere (the
  // composer, a field) never starts a review.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!matchesShortcut(e, 'reviewCode')) return;
      const focus = document.activeElement;
      const inPane = !!(paneRef.current && focus && paneRef.current.contains(focus));
      if (subject && inPane) {
        e.preventDefault();
        setRunRequest(n => n + 1);
        return;
      }
      if (isTextEntry(focus)) return;
      const selected = window.getSelection();
      const node = selected && !selected.isCollapsed ? selected.anchorNode : focus;
      if (!node) return;
      for (const source of sources.current) {
        if (source.root.contains(node)) {
          e.preventDefault();
          open(source.subject(selected?.toString() ?? ''));
          return;
        }
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [subject, open]);

  const value = useMemo<CodeReviewValue>(
    () => ({ open, close, subject, review, setReview, design, openDesign, setSelection, runs, awaitCheck, register, runRequest, paneRef }),
    [open, close, subject, review, design, openDesign, setSelection, runs, awaitCheck, register, runRequest],
  );
  return <CodeReviewContext.Provider value={value}>{children}</CodeReviewContext.Provider>;
}
