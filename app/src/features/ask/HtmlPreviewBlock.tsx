import { useEffect, useState } from 'react';
import type React from 'react';
import { cn } from '../../lib/utils';
import { HTML_PREVIEW_HEIGHT_CLASS, HTML_PREVIEW_SANDBOX, previewDocument } from './htmlPreview';
import { tr } from '../../i18n/tr';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

/** How long the preview waits for the text to stop changing (a streaming answer) before drawing. */
const SETTLE_MS = 400;

interface HtmlPreviewBlockProps {
  code: string;
  language: string;
  /** The code block's own buttons (Review this, Copy). */
  actions: React.ReactNode;
  /** The highlighted source, shown with Raw. */
  raw: React.ReactNode;
  codeRef?: React.Ref<HTMLDivElement>;
}

/**
 * An HTML block in an answer: the page drawn inline (Preview, the default) or its source
 * (Raw). The page runs no scripts and loads nothing (`htmlPreview`); it is the answer's
 * own block, never an artifact (only a requested make is one).
 */
export function HtmlPreviewBlock({ code, language, actions, raw, codeRef }: HtmlPreviewBlockProps) {
  const [view, setView] = useState<'preview' | 'raw'>('preview');
  // Drawn once the text settles, so a streaming page is not reloaded on every token.
  const [shown, setShown] = useState(code);
  useEffect(() => {
    if (code === shown) return;
    const timer = window.setTimeout(() => setShown(code), SETTLE_MS);
    return () => window.clearTimeout(timer);
  }, [code, shown]);

  const tab = (id: 'preview' | 'raw', label: string) => (
    <button
      type="button"
      role="radio"
      aria-checked={view === id}
      data-html-view={id}
      tabIndex={view === id ? 0 : -1}
      onClick={() => setView(id)}
      className={cn(
        'h-6 px-2 rounded-md text-caption font-medium transition-colors duration-micro',
        view === id ? 'bg-shodh-surface text-shodh-text shadow-sm' : 'text-shodh-text-muted hover:text-shodh-text',
        FOCUS_RING,
      )}
    >
      {label}
    </button>
  );

  return (
    <div data-testid="html-preview">
      <div className="flex items-center justify-between gap-2 ps-3 pe-1.5 h-8 border-b border-shodh-border-subtle bg-shodh-raised">
        <span className="font-mono text-caption uppercase tracking-wider text-shodh-text-muted" data-answer-ui="">{language}</span>
        <span className="inline-flex items-center gap-1" data-answer-ui="">
          <span
            role="radiogroup"
            aria-label={tr('ask.htmlPreviewBlock.show')}
            className="inline-flex items-center gap-0.5 p-0.5 rounded-lg bg-shodh-raised-2"
            onKeyDown={e => {
              if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
              e.preventDefault();
              const next = view === 'preview' ? 'raw' : 'preview';
              setView(next);
              e.currentTarget.querySelector<HTMLButtonElement>(`[data-html-view="${next}"]`)?.focus();
            }}
          >
            {tab('preview', 'Preview')}
            {tab('raw', 'Raw')}
          </span>
          {actions}
        </span>
      </div>
      {view === 'preview' ? (
        <iframe
          title={tr('ask.htmlPreviewBlock.htmlPreview')}
          sandbox={HTML_PREVIEW_SANDBOX}
          referrerPolicy="no-referrer"
          srcDoc={previewDocument(shown)}
          className={cn('block w-full bg-white resize-y', HTML_PREVIEW_HEIGHT_CLASS)}
        />
      ) : (
        <div ref={codeRef}>{raw}</div>
      )}
    </div>
  );
}

export default HtmlPreviewBlock;
