import React, { useCallback, useEffect, useId, useLayoutEffect, useMemo, useState } from 'react';
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
import { tabPastWindow, useWindowedList } from '../../hooks/useWindowedList';
import { gridColumns, gridRows, rowOfCard } from '../../lib/windowedRows';

/** The creations grid: a card's least width, the space between cards (`gap-3`), a row's height before it is measured. */
const CARD_MIN_PX = 280;
const CARD_GAP_PX = 12;
const CARD_ROW_ESTIMATE = 96;

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

  // The grid draws only the rows of cards in view (useWindowedList), in the page's scroller.
  const [pageNode, setPageNode] = useState<HTMLDivElement | null>(null);
  const [gridNode, setGridNode] = useState<HTMLDivElement | null>(null);
  const [gridWidth, setGridWidth] = useState(0);
  useLayoutEffect(() => {
    if (!gridNode) return;
    const measure = () => setGridWidth(gridNode.clientWidth);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(gridNode);
    return () => observer.disconnect();
  }, [gridNode]);
  const columns = gridColumns(gridWidth, CARD_MIN_PX, CARD_GAP_PX);
  const cardRows = useMemo(() => gridRows(shown.length, columns), [shown.length, columns]);
  const windowed = useWindowedList({
    count: cardRows.length,
    scroller: pageNode,
    list: gridNode,
    estimate: CARD_ROW_ESTIMATE,
    getKey: index => shown[cardRows[index].first].id,
    gap: CARD_GAP_PX,
  });
  const revealCardRow = windowed.reveal;

  // Tab past the last card drawn goes on to the next card, drawn first.
  const onGridKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>) => tabPastWindow(e, shown.length, card => rowOfCard(card, columns), revealCardRow),
    [shown.length, columns, revealCardRow],
  );

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
    <PageFrame ref={setPageNode} title="Artifacts" lede="Everything Shodh made">

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
              <Search className="w-3.5 h-3.5 absolute start-2.5 top-1/2 -translate-y-1/2 text-shodh-text-faint" aria-hidden="true" />
              <input
                id={searchId}
                type="search"
                value={text}
                onChange={e => setText(e.target.value)}
                placeholder="Search creations"
                className={cn('h-8 w-64 ps-8 pe-2.5 rounded-lg border border-shodh-border bg-shodh-surface text-small text-shodh-text placeholder:text-shodh-text-faint', FOCUS_RING)}
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
            <div
              ref={setGridNode}
              role="list"
              aria-label="Creations"
              onKeyDown={onGridKeyDown}
              className="relative"
              style={{ height: windowed.total }}
            >
              {windowed.rows.map(row => {
                const { first, last } = cardRows[row.index];
                return (
              <div
                key={row.key}
                ref={windowed.virtualizer.measureElement}
                data-index={row.index}
                role="presentation"
                className="absolute start-0 end-0 grid gap-3"
                style={{ top: row.top, gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` }}
              >
              {shown.slice(first, last + 1).map((creation, offset) => (
                <div key={creation.id} role="listitem" data-card-index={first + offset}>
                  <button
                    type="button"
                    onClick={() => setSelection(prev => ({ id: creation.id, version: null, pane: 'preview', showPreview: false, seq: (prev?.seq ?? 0) + 1 }))}
                    className={cn(
                      'w-full h-full text-start flex flex-col gap-1.5 rounded-xl border border-shodh-border bg-shodh-surface px-4 py-3 hover:border-shodh-border-strong hover:bg-shodh-raised transition-colors duration-micro',
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
                </div>
              ))}
              </div>
                );
              })}
            </div>
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
