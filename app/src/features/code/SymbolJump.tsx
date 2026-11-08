import { useCallback, useEffect, useId, useRef, useState } from 'react';
import type { KeyboardEvent as ReactKeyboardEvent } from 'react';
import { ListTree } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { ariaKeyShortcuts, IS_MAC_PLATFORM, matchesShortcut } from '../../lib/shortcuts';
import { Kbd } from '../../components/ui/Kbd';
import { codeMapApi, errorText } from '../codeIntel/api';
import type { SymbolHit } from '../codeIntel/api';
import { symbolRow } from '../codeIntel/codeMapFormat';
import { isTextEntry, useCodeReview } from './CodeReviewContext';
import { codeApi } from './api';
import { languageOf } from './diffModel';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-ground';
/** Typed this long without a key, the project is searched. */
const SEARCH_DELAY_MS = 150;

interface SymbolJumpProps {
  workspaceId: string;
  /** The file the code pane shows. */
  path: string;
}

/**
 * The code pane's symbol jump: the file's symbols from the code map, or with a
 * few letters typed the project's, and Enter goes to one (another file opens
 * at its lines). The registry's shortcut opens it while the pane shows a file.
 */
export function SymbolJump({ workspaceId, path }: SymbolJumpProps) {
  const pane = useCodeReview();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [hits, setHits] = useState<SymbolHit[]>([]);
  const [active, setActive] = useState(0);
  const [note, setNote] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const listId = useId();
  const generation = useRef(0);

  // The shortcut opens it, except while the user types in a field outside the
  // pane (the composer keeps its keys and its focus).
  const paneRef = pane?.paneRef;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!matchesShortcut(e, 'goToSymbol')) return;
      const focus = document.activeElement;
      const inPane = !!(paneRef?.current && focus && paneRef.current.contains(focus));
      if (!inPane && isTextEntry(focus)) return;
      e.preventDefault();
      setOpen(true);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [paneRef]);

  useEffect(() => {
    if (open) inputRef.current?.focus();
    else setQuery('');
  }, [open]);

  // The file's symbols with nothing typed; the project's with a few letters.
  useEffect(() => {
    if (!open) return;
    const mine = ++generation.current;
    const typed = query.trim();
    const timer = window.setTimeout(
      () => {
        const read = typed
          ? codeMapApi.find(workspaceId, typed)
          : codeMapApi.outline(workspaceId, path).then(outline => outline.files.flatMap(f => f.symbols));
        read
          .then(found => {
            if (mine !== generation.current) return;
            setHits(found);
            setActive(0);
            setNote(found.length === 0 ? (typed ? `No symbol starts with ${typed}.` : 'The code map has no symbols for this file yet.') : null);
          })
          .catch(err => {
            if (mine !== generation.current) return;
            setHits([]);
            setNote(errorText(err));
          });
      },
      typed ? SEARCH_DELAY_MS : 0,
    );
    return () => window.clearTimeout(timer);
  }, [open, query, workspaceId, path]);

  const go = useCallback(
    (hit: SymbolHit) => {
      if (!pane) return;
      const selection = { path: hit.path, side: 'new' as const, from: hit.line, to: Math.max(hit.line, Math.min(hit.endLine, hit.line + 60)) };
      setOpen(false);
      if (hit.path === path) {
        pane.setSelection(selection);
        return;
      }
      codeApi
        .file(workspaceId, hit.path)
        .then(file =>
          pane.open({
            kind: 'file',
            title: hit.path,
            path: hit.path,
            text: file.text,
            language: file.language === 'text' ? languageOf(hit.path) : file.language,
            firstLine: 1,
            workspaceId,
            selection,
          }),
        )
        .catch(err => notify.error('The file could not be opened', { description: errorText(err) }));
    },
    [pane, path, workspaceId],
  );

  const onKeyDown = (e: ReactKeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      if (hits.length === 0) return;
      const delta = e.key === 'ArrowDown' ? 1 : -1;
      setActive(i => (i + delta + hits.length) % hits.length);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const hit = hits[active];
      if (hit) go(hit);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
      buttonRef.current?.focus();
    }
  };

  if (!open) {
    return (
      <button
        ref={buttonRef}
        type="button"
        onClick={() => setOpen(true)}
        aria-keyshortcuts={ariaKeyShortcuts('goToSymbol', IS_MAC_PLATFORM)}
        className={cn(
          'self-start inline-flex items-center gap-1.5 h-7 px-2.5 rounded-md border border-shodh-border bg-shodh-raised text-caption text-shodh-text hover:bg-shodh-raised-2 transition-colors duration-micro',
          FOCUS_RING,
        )}
      >
        <ListTree className="w-3.5 h-3.5" aria-hidden="true" />
        Go to symbol
        <Kbd shortcut="goToSymbol" className="ml-1" />
      </button>
    );
  }

  const activeId = hits[active] ? `${listId}-${active}` : undefined;
  return (
    <div className="flex flex-col gap-1.5 rounded-lg border border-shodh-border bg-shodh-surface p-2">
      <label className="flex items-center gap-2">
        <ListTree className="w-3.5 h-3.5 shrink-0 text-shodh-text-muted" aria-hidden="true" />
        <span className="sr-only">Go to symbol</span>
        <input
          ref={inputRef}
          type="text"
          role="combobox"
          aria-expanded="true"
          aria-controls={listId}
          aria-activedescendant={activeId}
          aria-autocomplete="list"
          value={query}
          onChange={e => setQuery(e.target.value)}
          onKeyDown={onKeyDown}
          onBlur={e => {
            if (!e.currentTarget.parentElement?.parentElement?.contains(e.relatedTarget as Node | null)) setOpen(false);
          }}
          placeholder="A symbol of this file, or type to search the project"
          className={cn('flex-1 min-w-0 h-7 px-2 rounded-md bg-shodh-ground border border-shodh-border text-small text-shodh-text', FOCUS_RING)}
        />
      </label>
      {note && <p className="m-0 px-1 text-caption text-shodh-text-muted">{note}</p>}
      <ul id={listId} role="listbox" aria-label="Symbols" className="m-0 p-0 list-none max-h-56 overflow-y-auto scrollbar-thin">
        {hits.map((hit, i) => {
          const row = symbolRow(hit);
          return (
            <li
              key={`${hit.path}:${hit.line}:${hit.name}`}
              id={`${listId}-${i}`}
              role="option"
              aria-selected={i === active}
              onMouseDown={e => e.preventDefault()}
              onClick={() => go(hit)}
              onMouseEnter={() => setActive(i)}
              className={cn(
                'flex items-baseline gap-2 px-2 py-1 rounded-md cursor-pointer',
                i === active ? 'bg-shodh-accent-soft text-shodh-text' : 'text-shodh-text-secondary',
              )}
            >
              <span className="min-w-0 truncate font-mono text-caption">{row.title}</span>
              <span className="ml-auto shrink-0 font-mono text-caption text-shodh-text-muted">{row.where}</span>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

export default SymbolJump;
