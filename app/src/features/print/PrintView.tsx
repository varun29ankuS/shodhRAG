import { Component, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { MessageContentRenderer } from '../ask/MessageContentRenderer';
import { PrintModeContext } from './printContext';
import { hitsFromSources, printDate, printStylesheet, sourceAnchor, sourceLine } from './printModel';
import type { PrintJob } from './printModel';

/** No DOM change for this long means diagrams, charts and figures are drawn. */
const QUIET_MS = 700;
/** Longest wait for the drawing to settle; it prints what is there then. */
const SETTLE_LIMIT_MS = 45_000;

/** Longest wait for an animation frame: frames stop in a window the system considers hidden. */
const FRAME_LIMIT_MS = 50;

function nextFrame(): Promise<void> {
  return new Promise(resolve => {
    const timer = window.setTimeout(resolve, FRAME_LIMIT_MS);
    requestAnimationFrame(() => {
      window.clearTimeout(timer);
      resolve();
    });
  });
}

/** Every image under `root` loaded (or failed: a broken image must not hold the print). */
function imagesLoaded(root: HTMLElement): Promise<void> {
  const pending = Array.from(root.querySelectorAll('img')).filter(img => !img.complete);
  return Promise.all(
    pending.map(img => new Promise<void>(resolve => {
      img.addEventListener('load', () => resolve(), { once: true });
      img.addEventListener('error', () => resolve(), { once: true });
    })),
  ).then(() => undefined);
}

/** Resolves once `root` has stopped changing for `QUIET_MS` (or after the limit). */
function settled(root: HTMLElement): Promise<void> {
  return new Promise(resolve => {
    let quiet: number | undefined;
    const done = () => {
      observer.disconnect();
      window.clearTimeout(quiet);
      window.clearTimeout(limit);
      resolve();
    };
    const observer = new MutationObserver(() => {
      window.clearTimeout(quiet);
      quiet = window.setTimeout(done, QUIET_MS);
    });
    observer.observe(root, { subtree: true, childList: true, attributes: true, characterData: true });
    quiet = window.setTimeout(done, QUIET_MS);
    const limit = window.setTimeout(done, SETTLE_LIMIT_MS);
  });
}

const noCitation = () => undefined;

/** Reports a render error of the printed content, so the export fails at once. */
class PrintBoundary extends Component<{ onError: (message: string) => void; children: ReactNode }, { failed: string | null }> {
  state = { failed: null as string | null };

  static getDerivedStateFromError(error: unknown) {
    return { failed: error instanceof Error ? error.message : String(error) };
  }

  componentDidCatch(error: unknown) {
    this.props.onError(`the content could not be drawn (${error instanceof Error ? error.message : String(error)})`);
  }

  render() {
    return this.state.failed ? <p role="alert">{`The content could not be drawn: ${this.state.failed}`}</p> : this.props.children;
  }
}

/**
 * The print view (`/print-view` in its own window): renders the print job of
 * this window with the answer renderer and a print stylesheet, waits until
 * fonts, figures and diagrams are drawn, then tells the app it can print.
 */
export default function PrintView() {
  const [job, setJob] = useState<PrintJob | null>(null);
  const [error, setError] = useState<string | null>(null);
  const rootRef = useRef<HTMLElement>(null);
  const reported = useRef(false);

  const report = useCallback((problem: string | null) => {
    if (reported.current) return;
    reported.current = true;
    invoke('print_job_ready', { error: problem }).catch(e => console.error('Reporting the print view failed:', e));
  }, []);

  // A render error anywhere in the page fails the export at once instead of at the timeout.
  useEffect(() => {
    const onError = (event: ErrorEvent) => report(event.message || 'a script error stopped the page');
    window.addEventListener('error', onError);
    return () => window.removeEventListener('error', onError);
  }, [report]);

  useEffect(() => {
    invoke<PrintJob>('print_job').then(
      value => {
        setJob(value);
        document.title = value.document.title;
      },
      e => {
        const message = e instanceof Error ? e.message : String(e);
        setError(message);
        report(message);
      },
    );
  }, [report]);

  useEffect(() => {
    const root = rootRef.current;
    if (!job || !root) return;
    let cancelled = false;
    (async () => {
      await document.fonts.ready;
      await nextFrame();
      await settled(root);
      await imagesLoaded(root);
      await nextFrame();
      await nextFrame();
      if (!cancelled) report(null);
    })().catch(e => report(e instanceof Error ? e.message : String(e)));
    return () => {
      cancelled = true;
    };
  }, [job, report]);

  const hits = useMemo(() => (job ? hitsFromSources(job.document.sources) : []), [job]);

  if (error) {
    return (
      <main className="p-8 text-shodh-text">
        <p role="alert">{`The print view could not open: ${error}`}</p>
      </main>
    );
  }
  if (!job) {
    return (
      <main className="p-8 text-shodh-text-muted">
        <p role="status">Preparing the page…</p>
      </main>
    );
  }
  const { document: doc, native } = job;
  const date = printDate(doc.createdAt);
  return (
    <PrintModeContext.Provider value={true}>
      <style>{printStylesheet()}</style>
      <main ref={rootRef} className="print-view mx-auto max-w-[700px] px-2 py-6 bg-shodh-ground text-shodh-text">
        {!native && (
          <p className="print-screen-only mb-6 rounded-lg border border-shodh-border bg-shodh-raised px-3 py-2 text-[13px] text-shodh-text-secondary" role="note">
            Choose “Save as PDF” (or “Microsoft Print to PDF”) as the printer in the print dialog to save this as a PDF. Close this window when you are done.
          </p>
        )}
        <header className="mb-6 flex flex-col gap-1 border-b border-shodh-border pb-4">
          <h1 className="m-0 text-[24px] font-bold leading-tight text-shodh-text">{doc.title}</h1>
          {doc.subtitle && <p className="m-0 text-[14px] text-shodh-text-secondary">{doc.subtitle}</p>}
          {date && <p className="m-0 text-[12.5px] text-shodh-text-muted">{date}</p>}
        </header>
        <PrintBoundary onError={report}>
          <MessageContentRenderer content={doc.markdown} hits={hits} onOpenCitation={noCitation} print />
        </PrintBoundary>
        {doc.sources.length > 0 && (
          <section aria-labelledby="print-sources" className="mt-8 border-t border-shodh-border pt-4" data-print-block="">
            <h2 id="print-sources" className="m-0 mb-3 text-[17px] font-semibold text-shodh-text">Sources</h2>
            <ol className="m-0 list-none p-0 flex flex-col gap-1.5 text-[13px] text-shodh-text-secondary">
              {doc.sources.map(source => (
                <li key={source.n} id={sourceAnchor(source.n)} className="break-words">
                  <span className="font-semibold tabular-nums text-shodh-text">{`[${source.n}]`}</span>{' '}
                  {sourceLine(source)}
                  {source.url && (
                    <>
                      {' — '}
                      <a href={source.url} className="underline">{source.url}</a>
                    </>
                  )}
                </li>
              ))}
            </ol>
          </section>
        )}
      </main>
    </PrintModeContext.Provider>
  );
}
