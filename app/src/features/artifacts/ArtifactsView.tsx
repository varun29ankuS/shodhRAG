import React, { useCallback, useEffect, useId, useMemo, useState } from 'react';
import { GitBranch, Hammer, MessageCircle, Search } from 'lucide-react';
import { cn } from '../../lib/utils';
import { relativeTime } from '../../utils/time';
import { VisualGallery } from '../visuals/VisualGallery';
import { useNavigationTarget } from '../agent/useNavigationTarget';
import { creationsApi, errorText, onCreationsChanged } from './api';
import { ArtifactPage } from './ArtifactPage';
import { STATUS_LABELS, filterCreations, selectionFor } from './model';
import type { ArtifactSelection, CodeChange, Creation } from './model';
import { PageFrame } from '../../components/shell/PageFrame';
import { ViewState } from '../../components/shell/ViewState';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

export type { ArtifactSelection } from './model';

interface ArtifactsViewProps {
  /** Open the chat that made a creation. */
  onOpenChat: (conversationId: string) => void;
  /** Change something: the creation's chat, with the composer started. */
  onChange: (conversationId: string) => void;
}

/**
 * The Artifacts tab: everything Shodh made. Creator mode's creations (each opens on its
 * page: preview, files, versions), then the diagrams, charts, tables and snippets of every
 * conversation (the gallery that was the Library's "Generated" section).
 */
export default function ArtifactsView({ onOpenChat, onChange }: ArtifactsViewProps) {
  const [creations, setCreations] = useState<Creation[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [text, setText] = useState('');
  const [selection, setSelection] = useState<ArtifactSelection | null>(null);
  const searchId = useId();
  const [codeChanges, setCodeChanges] = useState<CodeChange[]>([]);

  // Code the agent wrote in Code mode (read once per visit; the branch lives in its chat).
  useEffect(() => {
    creationsApi
      .codeChanges()
      .then(setCodeChanges)
      .catch(e => console.error('Code changes unavailable:', e));
  }, []);

  const load = useCallback(() => {
    creationsApi
      .list()
      .then(list => {
        setCreations(list);
        setError(null);
      })
      .catch(e => setError(errorText(e)));
  }, []);

  useEffect(() => {
    load();
    let disposed = false;
    let unlisten: (() => void) | null = null;
    onCreationsChanged(() => load())
      .then(fn => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(e => console.error('Failed to listen for creations-changed:', e));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [load]);

  // The assistant showed a creation (open_artifact), or a Creator answer started building.
  useNavigationTarget('artifact', target => {
    setSelection(prev => selectionFor(target, prev));
  });

  const shown = useMemo(() => filterCreations(creations ?? [], text), [creations, text]);

  if (selection) {
    return (
      <ArtifactPage
        key={selection.id}
        selection={selection}
        onBack={() => setSelection(null)}
        onOpenChat={onOpenChat}
        onChange={onChange}
        onDeleted={() => {
          setSelection(null);
          load();
        }}
      />
    );
  }

  return (
    <PageFrame title="Artifacts" lede="Everything Shodh made">

        <section aria-labelledby="artifacts-creations" className="flex flex-col gap-3">
          <div className="flex items-end justify-between gap-3 flex-wrap">
            <div className="flex flex-col gap-0.5">
              <h2 id="artifacts-creations" className="text-caption font-semibold uppercase tracking-[0.08em] text-shodh-text-faint">
                Creations
              </h2>
              <p className="m-0 text-small text-shodh-text-muted">
                Apps, games and pages Creator mode built, each in its own folder in Documents/Shodh Creations.
              </p>
            </div>
            <label htmlFor={searchId} className="relative">
              <span className="sr-only">Search creations</span>
              <Search className="w-3.5 h-3.5 absolute left-2.5 top-1/2 -translate-y-1/2 text-shodh-text-faint" aria-hidden="true" />
              <input
                id={searchId}
                type="search"
                value={text}
                onChange={e => setText(e.target.value)}
                placeholder="Search creations"
                className={cn('h-8 w-64 pl-8 pr-2.5 rounded-lg border border-shodh-border bg-shodh-surface text-small text-shodh-text placeholder:text-shodh-text-faint', FOCUS_RING)}
              />
            </label>
          </div>

          {error && <ViewState kind="error" title="Creations could not be read" detail={error} />}
          {creations === null && !error ? (
            <ViewState kind="loading" title="Loading creations" />
          ) : creations !== null && creations.length === 0 ? (
            <ViewState kind="empty" icon={Hammer} title="Nothing built yet" detail="Ask for an app, a game or a page in Creator mode." />
          ) : shown.length === 0 ? (
            <p className="m-0 text-small text-shodh-text-muted">No creation matches “{text.trim()}”.</p>
          ) : (
            <ul className="grid gap-3 grid-cols-[repeat(auto-fill,minmax(280px,1fr))] list-none p-0 m-0">
              {shown.map(creation => (
                <li key={creation.id}>
                  <button
                    type="button"
                    onClick={() => setSelection(prev => ({ id: creation.id, version: null, pane: 'preview', showPreview: false, seq: (prev?.seq ?? 0) + 1 }))}
                    className={cn(
                      'w-full h-full text-left flex flex-col gap-1.5 rounded-xl border border-shodh-border bg-shodh-surface px-4 py-3 hover:border-shodh-border-strong hover:bg-shodh-raised transition-colors duration-micro',
                      FOCUS_RING,
                    )}
                  >
                    <span className="flex items-center gap-2 min-w-0">
                      <Hammer className="w-3.5 h-3.5 shrink-0 text-shodh-text-secondary" aria-hidden="true" />
                      <span className="text-small font-semibold text-shodh-text truncate">{creation.title}</span>
                    </span>
                    {creation.summary && (
                      <span className="text-caption text-shodh-text-muted line-clamp-2">{creation.summary}</span>
                    )}
                    <span className="text-caption text-shodh-text-faint">
                      {STATUS_LABELS[creation.status]} · {creation.versions.length}{' '}
                      {creation.versions.length === 1 ? 'version' : 'versions'} · {relativeTime(creation.updatedAt)}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </section>

        {codeChanges.length > 0 && (
          <section aria-labelledby="artifacts-code" className="flex flex-col gap-3">
            <div className="flex flex-col gap-0.5">
              <h2 id="artifacts-code" className="text-caption font-semibold uppercase tracking-[0.08em] text-shodh-text-faint">
                Code changes
              </h2>
              <p className="m-0 text-small text-shodh-text-muted">
                Code the assistant wrote in Code mode, each on its own branch of the workspace folder. Review or discard it in its chat.
              </p>
            </div>
            <ul className="list-none m-0 p-0 flex flex-col gap-1.5">
              {codeChanges.map(change => (
                <li key={change.conversationId} className="flex items-center gap-3 rounded-xl border border-shodh-border-subtle px-4 py-2.5">
                  <GitBranch className="w-3.5 h-3.5 shrink-0 text-shodh-text-secondary" aria-hidden="true" />
                  <span className="flex-1 min-w-0 flex flex-col">
                    <span className="text-small font-medium text-shodh-text truncate">{change.branch}</span>
                    <span className="text-caption text-shodh-text-muted truncate">
                      {change.folder} · from {change.base} · {relativeTime(change.createdAt)}
                    </span>
                  </span>
                  <button
                    type="button"
                    onClick={() => onOpenChat(change.conversationId)}
                    className={cn('h-8 px-2.5 inline-flex items-center gap-1.5 rounded-lg text-small text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text', FOCUS_RING)}
                  >
                    <MessageCircle className="w-3.5 h-3.5" aria-hidden="true" />
                    Open chat
                  </button>
                </li>
              ))}
            </ul>
          </section>
        )}

        <section aria-labelledby="artifacts-generated" className="flex flex-col gap-3">
          <div className="flex flex-col gap-0.5">
            <h2 id="artifacts-generated" className="text-caption font-semibold uppercase tracking-[0.08em] text-shodh-text-faint">
              Generated
            </h2>
            <p className="m-0 text-small text-shodh-text-muted">
              Diagrams, charts, sketches, plots, simulations, equations and tables from every conversation, with your snippets.
            </p>
          </div>
          <VisualGallery conversationId={null} />
        </section>
    </PageFrame>
  );
}
