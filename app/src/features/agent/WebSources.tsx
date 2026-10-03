import React from 'react';
import { Globe } from 'lucide-react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import type { WebSource } from './reducer';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-ground';

function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

/** Open a web source in the system browser (never inside the app). */
export async function openExternal(url: string): Promise<void> {
  try {
    await openUrl(url);
  } catch (error) {
    notify.error('Could not open the link', {
      description: error instanceof Error ? error.message : String(error),
    });
  }
}

interface WebSourcesProps {
  sources: readonly WebSource[];
  /** Provider name, e.g. "OpenRouter web search". */
  provider: string | null;
}

/**
 * Web results of a step: numbered like document passages but marked as web
 * content (globe, outlined numbers, host shown), each opening in the
 * system browser.
 */
export function WebSources({ sources, provider }: WebSourcesProps) {
  return (
    <div className="mt-1.5 flex flex-col gap-1 font-sans">
      <p className="flex items-center gap-1.5 text-[11px] text-shodh-text-faint">
        <Globe className="w-3 h-3" aria-hidden="true" />
        {provider ? `Web content via ${provider} — untrusted` : 'Web content — untrusted'}
      </p>
      <ol className="list-none p-0 m-0 flex flex-col gap-1" aria-label="Web sources">
        {sources.map(s => (
          <li key={s.n} className="min-w-0">
            <button
              type="button"
              onClick={() => void openExternal(s.url)}
              title={s.url}
              className={cn('flex items-baseline gap-1.5 min-w-0 text-left rounded-sm hover:underline', FOCUS_RING)}
            >
              <span className="shrink-0 px-1 rounded-[4px] border border-dashed border-shodh-info text-shodh-info text-[10.5px] font-bold tabular-nums">
                {s.n}
              </span>
              <span className="text-shodh-text truncate">{s.title}</span>
              <span className="shrink-0 text-shodh-text-faint text-[11px]">{hostOf(s.url)}</span>
              <span className="sr-only">(opens in your browser)</span>
            </button>
            {s.snippet && (
              <p className="ml-7 text-[11.5px] text-shodh-text-muted line-clamp-2 whitespace-normal">{s.snippet}</p>
            )}
          </li>
        ))}
      </ol>
    </div>
  );
}

/**
 * Attribution a search provider requires next to its results (Google's
 * search suggestions for Gemini grounding). Rendered in a fully sandboxed
 * frame (no scripts, same-origin, popups or top navigation), so the
 * provider's HTML cannot reach the app or its IPC.
 */
export function AttributionFrame({ html }: { html: string }) {
  return (
    <iframe
      title="Google Search suggestions"
      sandbox=""
      referrerPolicy="no-referrer"
      srcDoc={html}
      className="mt-1 ml-[17px] w-[calc(100%-17px)] h-[56px] rounded-md border border-shodh-border-subtle bg-white"
    />
  );
}

export default WebSources;
