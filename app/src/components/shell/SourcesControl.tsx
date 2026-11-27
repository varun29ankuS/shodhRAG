import { Database, Globe } from 'lucide-react';
import { cn } from '../../lib/utils';
import { tr } from '../../i18n/tr';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-ground';

interface SourcesControlProps {
  /** What answers search (`scopeChip` on Ask, `librarySourcesChip` elsewhere). */
  label: string;
  title: string;
  /** Open where these sources are managed: the workspace's sources, or the Library. */
  onOpen: () => void;
  /** "Search all my library" for the next question of a workspace chat; null outside one. */
  searchAll: { on: boolean; toggle: () => void } | null;
}

/**
 * The top bar's Sources: what answers search, on every view, in one place. It opens where
 * the sources are managed; in a workspace chat, "All library" beside it widens the next
 * question to the whole library.
 */
export function SourcesControl({ label, title, onOpen, searchAll }: SourcesControlProps) {
  return (
    <div className="flex items-center gap-1.5 min-w-0" data-testid="topbar-sources">
      {searchAll && (
        <button
          type="button"
          aria-pressed={searchAll.on}
          onClick={searchAll.toggle}
          title={tr('shell.sourcesControl.nextQuestionOnlySearchYour')}
          data-testid="topbar-search-all"
          className={cn(
            'h-[26px] px-2.5 shrink-0 whitespace-nowrap inline-flex items-center gap-1 rounded-full text-caption font-medium border transition-colors duration-micro',
            searchAll.on
              ? 'bg-shodh-accent-soft border-shodh-accent text-shodh-accent-text'
              : 'border-shodh-border text-shodh-text-muted hover:text-shodh-text hover:bg-shodh-raised',
            FOCUS_RING,
          )}
        >
          <Globe className="w-3 h-3" aria-hidden="true" />
          {tr('shell.sourcesControl.allLibrary')}
        </button>
      )}
      <button
        type="button"
        onClick={onOpen}
        title={title}
        aria-label={`Sources: ${label}`}
        data-testid="topbar-sources-open"
        className={cn(
          'h-[26px] px-2.5 min-w-0 inline-flex items-center gap-1 rounded-full border border-shodh-border text-caption text-shodh-text-muted hover:text-shodh-text hover:bg-shodh-raised transition-colors duration-micro',
          FOCUS_RING,
        )}
      >
        <Database className="w-3 h-3 shrink-0" aria-hidden="true" />
        <span className="truncate max-w-[220px]">{label}</span>
      </button>
    </div>
  );
}

export default SourcesControl;
