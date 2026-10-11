import React, { useEffect, useId, useState } from 'react';
import { AlertTriangle, Loader2, Search } from 'lucide-react';
import { cn } from '../../lib/utils';
import { SnippetCard } from './SnippetCard';
import { sortSnippets } from './snippetModel';
import { BUTTON, FOCUS_RING } from './ui';
import { useSnippetList } from './useSnippets';

const SEARCH_DELAY_MS = 250;
const LIMIT = 200;

/**
 * Saved snippets as a grid with search (meaning and words). `filePath`
 * limits it to one paper; without it every snippet is shown.
 */
export function SnippetShelf({ filePath = null, emptyText }: { filePath?: string | null; emptyText?: string }) {
  const searchId = useId();
  const [query, setQuery] = useState('');
  const [debounced, setDebounced] = useState('');
  useEffect(() => {
    const timer = window.setTimeout(() => setDebounced(query.trim()), SEARCH_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [query]);
  const { state, reload, replace } = useSnippetList({ filePath, text: debounced || null, limit: LIMIT });
  const items = state.status === 'ready' ? (debounced ? state.items : sortSnippets(state.items)) : [];

  return (
    <div className="flex flex-col gap-3 min-h-0">
      <div className="relative max-w-[360px]">
        <Search className="absolute left-2.5 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-shodh-text-faint" aria-hidden="true" />
        <label htmlFor={searchId} className="sr-only">Search snippets</label>
        <input
          id={searchId}
          type="search"
          value={query}
          onChange={e => setQuery(e.target.value)}
          placeholder="Search snippets by meaning or words"
          className="w-full h-8 pl-8 pr-2.5 rounded-lg border border-shodh-border bg-shodh-surface-2 text-[12.5px] text-shodh-text placeholder:text-shodh-text-faint focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        />
      </div>
      <p className="sr-only" role="status" aria-live="polite">
        {state.status === 'ready' ? `${items.length} ${items.length === 1 ? 'snippet' : 'snippets'}` : ''}
      </p>
      {state.status === 'loading' ? (
        <div className="flex items-center gap-2 py-8 justify-center text-[12.5px] text-shodh-text-muted">
          <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
          Loading snippets…
        </div>
      ) : state.status === 'error' ? (
        <div role="alert" className="flex items-start gap-2 rounded-xl border border-shodh-border bg-shodh-surface px-4 py-3 text-[12.5px] text-shodh-text-secondary">
          <AlertTriangle className="w-4 h-4 mt-0.5 shrink-0 text-shodh-warning" aria-hidden="true" />
          <span className="flex-1">{`Snippets could not be loaded: ${state.message}`}</span>
          <button type="button" onClick={reload} className={cn(BUTTON, FOCUS_RING)}>
            Try again
          </button>
        </div>
      ) : items.length === 0 ? (
        <p className="py-6 text-center text-[13px] text-shodh-text-muted">
          {debounced
            ? 'No snippet matches.'
            : emptyText ?? 'No snippets yet. In a PDF, press S (or the scissors button) and drag a rectangle to save a figure, table or passage.'}
        </p>
      ) : (
        <ul className="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-3" aria-label="Snippets">
          {items.map(snippet => (
            <SnippetCard key={snippet.id} snippet={snippet} showFile={!filePath} onChange={next => replace(snippet.id, next)} />
          ))}
        </ul>
      )}
    </div>
  );
}
