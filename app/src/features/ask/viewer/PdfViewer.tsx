import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { PDFDocumentLoadingTask, PDFDocumentProxy, PDFPageProxy, RenderTask } from 'pdfjs-dist';
import { ChevronDown, ChevronUp, Loader2, Minus, MoveHorizontal, Plus } from 'lucide-react';
import { cn } from '../../../lib/utils';
import type { PageSpan } from '../types';
import { findPassage, matchScore, prepareHaystack, type PassageMatch, type PreparedHaystack, type TextRange } from './passageMatch';
import { isRenderCancelled, loadPdfJs, openPdf } from './pdfjs';
import { readSourceBytes, scrollBehavior } from './sourceAccess';
import type { LocateResult } from './viewerTypes';
import { VIEWER_FOCUS_RING } from './viewerTypes';

type TextContent = Awaited<ReturnType<PDFPageProxy['getTextContent']>>;
type TextLayerInstance = InstanceType<Awaited<ReturnType<typeof loadPdfJs>>['TextLayer']>;

interface PageText {
  content: TextContent;
  /** `str` of every text item, in the order pdf.js's TextLayer creates spans. */
  strings: string[];
  haystack: PreparedHaystack;
}

interface PageSize {
  width: number;
  height: number;
}

interface HighlightTarget {
  page: number;
  ranges: TextRange[];
  token: number;
}

interface Rect {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** Cap on canvas backing-store pixels per page (memory guard at high DPR). */
const MAX_CANVAS_PIXELS = 16_000_000;
const MIN_SCALE = 0.25;
const MAX_SCALE = 5;
const ZOOM_STEP = 1.2;
const PAGE_GAP = 12;
const PAGE_PADDING = 16;
/** A page match this good ends the search early. */
const CONFIDENT_COVERAGE = 0.9;

function clampScale(scale: number): number {
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, scale));
}

/** Merge per-span rects that sit on the same line and touch. */
function mergeRects(rects: Rect[]): Rect[] {
  const sorted = [...rects].sort((a, b) => a.top - b.top || a.left - b.left);
  const out: Rect[] = [];
  for (const r of sorted) {
    const prev = out[out.length - 1];
    if (
      prev &&
      Math.abs(prev.top - r.top) < Math.max(2, prev.height * 0.4) &&
      Math.abs(prev.height - r.height) < Math.max(3, prev.height * 0.5) &&
      r.left <= prev.left + prev.width + 6
    ) {
      const right = Math.max(prev.left + prev.width, r.left + r.width);
      const bottom = Math.max(prev.top + prev.height, r.top + r.height);
      prev.left = Math.min(prev.left, r.left);
      prev.top = Math.min(prev.top, r.top);
      prev.width = right - prev.left;
      prev.height = bottom - prev.top;
    } else {
      out.push({ ...r });
    }
  }
  return out;
}

interface PdfPageViewProps {
  doc: PDFDocumentProxy;
  pageNumber: number;
  scale: number;
  size: PageSize;
  active: boolean;
  getText: (page: number) => Promise<PageText>;
  ranges: TextRange[] | null;
  highlightToken: number | null;
  /** `top` is the first highlight's offset in the page, or null if none could be drawn. */
  onHighlightRendered: (page: number, token: number, top: number | null) => void;
  registerElement: (page: number, el: HTMLDivElement | null) => void;
}

function PdfPageView({
  doc,
  pageNumber,
  scale,
  size,
  active,
  getText,
  ranges,
  highlightToken,
  onHighlightRendered,
  registerElement,
}: PdfPageViewProps) {
  const pageRef = useRef<HTMLDivElement>(null);
  const canvasHostRef = useRef<HTMLDivElement>(null);
  const textHostRef = useRef<HTMLDivElement>(null);
  const [layer, setLayer] = useState<{ divs: HTMLElement[]; strings: string[] } | null>(null);
  /** Canvas state: drawing (spinner), drawn, or failed (message). */
  const [stage, setStage] = useState<'idle' | 'drawing' | 'drawn' | 'failed'>('idle');
  /** Text layer could not be used for highlighting (failed or misaligned). */
  const [layerUnusable, setLayerUnusable] = useState(false);
  /** null until computed for the current layer and ranges. */
  const [rects, setRects] = useState<Rect[] | null>(null);

  const setPageRef = useCallback(
    (el: HTMLDivElement | null) => {
      pageRef.current = el;
      registerElement(pageNumber, el);
    },
    [pageNumber, registerElement],
  );

  useEffect(() => {
    const canvasHost = canvasHostRef.current;
    const textHost = textHostRef.current;
    if (!active || !canvasHost || !textHost) {
      canvasHost?.replaceChildren();
      textHost?.replaceChildren();
      setLayer(null);
      setLayerUnusable(false);
      setStage('idle');
      return;
    }
    let cancelled = false;
    let renderTask: RenderTask | null = null;
    let textLayer: TextLayerInstance | null = null;
    let drawn = false;
    setStage(prev => (prev === 'drawn' ? 'drawn' : 'drawing'));
    setLayerUnusable(false);

    (async () => {
      const pdfjs = await loadPdfJs();
      const page = await doc.getPage(pageNumber);
      if (cancelled) return;
      const viewport = page.getViewport({ scale });
      const ratio = Math.max(
        0.5,
        Math.min(window.devicePixelRatio || 1, Math.sqrt(MAX_CANVAS_PIXELS / Math.max(1, viewport.width * viewport.height))),
      );
      // A fresh canvas per render: pdf.js refuses concurrent renders on one
      // canvas, and swapping on completion avoids a blank flash while zooming.
      const canvas = document.createElement('canvas');
      canvas.width = Math.max(1, Math.floor(viewport.width * ratio));
      canvas.height = Math.max(1, Math.floor(viewport.height * ratio));
      canvas.setAttribute('aria-hidden', 'true');
      const context = canvas.getContext('2d');
      if (!context) throw new Error('Canvas 2D context unavailable');
      renderTask = page.render({
        canvas,
        canvasContext: context,
        viewport,
        transform: ratio !== 1 ? [ratio, 0, 0, ratio, 0, 0] : undefined,
      });
      await renderTask.promise;
      if (cancelled) return;
      canvasHost.replaceChildren(canvas);
      drawn = true;
      setStage('drawn');

      const text = await getText(pageNumber);
      if (cancelled) return;
      const container = document.createElement('div');
      container.className = 'pdf-text-layer';
      textLayer = new pdfjs.TextLayer({ textContentSource: text.content, container, viewport });
      await textLayer.render();
      if (cancelled) return;
      textHost.replaceChildren(container);
      const divs = textLayer.textDivs;
      const strings = textLayer.textContentItemsStr;
      // Highlight offsets are computed on `text.strings`; only trust the
      // layer when it produced exactly the same runs.
      const aligned = divs.length === strings.length && strings.length === text.strings.length && strings.every((s, i) => s === text.strings[i]);
      setLayer(aligned ? { divs, strings } : null);
      setLayerUnusable(!aligned);
    })().catch(error => {
      if (cancelled || isRenderCancelled(error)) return;
      // A text-layer failure after the canvas was drawn leaves the page
      // readable; only highlighting is lost.
      if (drawn) {
        setLayer(null);
        setLayerUnusable(true);
      } else {
        setStage('failed');
      }
    });

    return () => {
      cancelled = true;
      renderTask?.cancel();
      textLayer?.cancel();
    };
  }, [active, doc, pageNumber, scale, getText]);

  useLayoutEffect(() => {
    const pageEl = pageRef.current;
    if (!layer || !ranges || ranges.length === 0 || !pageEl) {
      setRects(null);
      return;
    }
    const origin = pageEl.getBoundingClientRect();
    const domRange = document.createRange();
    const found: Rect[] = [];
    let offset = 0;
    layer.strings.forEach((str, index) => {
      const start = offset;
      const end = offset + str.length;
      offset = end;
      const node = layer.divs[index]?.firstChild;
      if (!node || node.nodeType !== Node.TEXT_NODE) return;
      const length = (node as Text).length;
      for (const r of ranges) {
        const a = Math.max(r.start, start);
        const b = Math.min(r.end, end);
        if (a >= b) continue;
        domRange.setStart(node, Math.min(a - start, length));
        domRange.setEnd(node, Math.min(b - start, length));
        for (const rect of Array.from(domRange.getClientRects())) {
          if (rect.width < 0.5 || rect.height < 0.5) continue;
          found.push({ left: rect.left - origin.left, top: rect.top - origin.top, width: rect.width, height: rect.height });
        }
      }
    });
    domRange.detach();
    setRects(mergeRects(found));
  }, [layer, ranges]);

  useEffect(() => {
    if (highlightToken === null || !ranges || ranges.length === 0) return;
    if (layerUnusable) {
      onHighlightRendered(pageNumber, highlightToken, null);
    } else if (rects !== null) {
      onHighlightRendered(pageNumber, highlightToken, rects.length > 0 ? rects.reduce((m, r) => Math.min(m, r.top), Infinity) : null);
    }
  }, [rects, layerUnusable, ranges, highlightToken, pageNumber, onHighlightRendered]);

  return (
    <div
      ref={setPageRef}
      data-page={pageNumber}
      className="pdf-page"
      style={{ width: size.width, height: size.height, ['--total-scale-factor' as string]: String(scale) }}
      role="region"
      aria-label={`Page ${pageNumber}`}
    >
      <div ref={canvasHostRef} className="absolute inset-0" />
      <div ref={textHostRef} className="absolute inset-0" />
      {rects && rects.length > 0 && (
        <div className="pdf-highlight-layer" aria-hidden="true">
          {rects.map((r, i) => (
            <div key={i} className="pdf-highlight" style={{ left: r.left - 1, top: r.top - 1, width: r.width + 2, height: r.height + 2 }} />
          ))}
        </div>
      )}
      {active && stage === 'drawing' && (
        <div className="absolute inset-0 flex items-center justify-center pointer-events-none" aria-hidden="true">
          <Loader2 className="w-5 h-5 animate-spin motion-reduce:animate-none text-zinc-500" />
        </div>
      )}
      {stage === 'failed' && (
        <div className="absolute inset-0 flex items-center justify-center p-4 text-center text-[12.5px] text-zinc-700">
          Page {pageNumber} could not be rendered.
        </div>
      )}
    </div>
  );
}

interface PdfViewerProps {
  filePath: string;
  passage: string;
  citedPages: PageSpan | null;
  onLocate: (result: LocateResult) => void;
  onFatal: (error: unknown) => void;
}

/**
 * Renders a PDF with pdf.js (canvas + text layer), lazily per visible page,
 * and highlights the cited passage on the page it was found.
 */
export function PdfViewer({ filePath, passage, citedPages, onLocate, onFatal }: PdfViewerProps) {
  const scrollerRef = useRef<HTMLDivElement>(null);
  const pageEls = useRef(new Map<number, HTMLDivElement>());
  const textCache = useRef(new Map<number, Promise<PageText>>());
  const [doc, setDoc] = useState<PDFDocumentProxy | null>(null);
  const [sizes, setSizes] = useState<PageSize[]>([]);
  const [zoom, setZoom] = useState<{ mode: 'fit' } | { mode: 'manual'; scale: number }>({ mode: 'fit' });
  const [fitScale, setFitScale] = useState(1);
  const [visible, setVisible] = useState<Set<number>>(() => new Set());
  const [currentPage, setCurrentPage] = useState(1);
  const [pageInput, setPageInput] = useState('1');
  const [target, setTarget] = useState<HighlightTarget | null>(null);
  const locateToken = useRef(0);
  const scrolledToken = useRef<number | null>(null);
  const anchorPage = useRef<number | null>(null);
  const observerRef = useRef<IntersectionObserver | null>(null);

  const onLocateRef = useRef(onLocate);
  const onFatalRef = useRef(onFatal);
  useEffect(() => {
    onLocateRef.current = onLocate;
    onFatalRef.current = onFatal;
  }, [onLocate, onFatal]);

  const scale = zoom.mode === 'fit' ? fitScale : zoom.scale;
  const numPages = doc?.numPages ?? 0;

  const currentPageRef = useRef(1);
  const zoomModeRef = useRef(zoom.mode);
  const fitInitialized = useRef(false);
  useEffect(() => {
    currentPageRef.current = currentPage;
    setPageInput(String(currentPage));
  }, [currentPage]);
  useEffect(() => {
    zoomModeRef.current = zoom.mode;
  }, [zoom.mode]);

  // Load the document (once per file).
  useEffect(() => {
    let cancelled = false;
    let task: PDFDocumentLoadingTask | null = null;
    textCache.current = new Map();
    fitInitialized.current = false;
    setDoc(null);
    setSizes([]);
    setTarget(null);

    (async () => {
      const bytes = await readSourceBytes(filePath);
      if (cancelled) return;
      task = await openPdf(bytes);
      if (cancelled) {
        void task.destroy();
        return;
      }
      const loaded = await task.promise;
      if (cancelled) return;
      const first = (await loaded.getPage(1)).getViewport({ scale: 1 });
      if (cancelled) return;
      setSizes(Array.from({ length: loaded.numPages }, () => ({ width: first.width, height: first.height })));
      setDoc(loaded);

      // Resolve real page sizes in the background so placeholders (and the
      // scroll offsets derived from them) match mixed-size documents.
      const resolved: PageSize[] = Array.from({ length: loaded.numPages }, () => ({ width: first.width, height: first.height }));
      for (let n = 2; n <= loaded.numPages; n += 1) {
        const vp = (await loaded.getPage(n)).getViewport({ scale: 1 });
        if (cancelled) return;
        resolved[n - 1] = { width: vp.width, height: vp.height };
        if (n % 50 === 0 || n === loaded.numPages) setSizes([...resolved]);
      }
    })().catch(error => {
      if (!cancelled) onFatalRef.current(error);
    });

    return () => {
      cancelled = true;
      if (task) void task.destroy();
    };
  }, [filePath]);

  const getText = useCallback(
    (page: number): Promise<PageText> => {
      if (!doc) return Promise.reject(new Error('Document not loaded'));
      let cached = textCache.current.get(page);
      if (!cached) {
        cached = doc
          .getPage(page)
          .then(p => p.getTextContent())
          .then(content => {
            const strings: string[] = [];
            for (const item of content.items) {
              if ('str' in item) strings.push(item.str);
            }
            return { content, strings, haystack: prepareHaystack(strings.join('')) };
          });
        cached.catch(() => textCache.current.delete(page));
        textCache.current.set(page, cached);
      }
      return cached;
    },
    [doc],
  );

  // Fit-to-width scale follows the panel width (including expand/collapse).
  useEffect(() => {
    const scroller = scrollerRef.current;
    if (!scroller || sizes.length === 0) return;
    const base = sizes[0];
    const update = () => {
      const width = scroller.clientWidth - PAGE_PADDING * 2;
      if (width <= 0) return;
      const next = clampScale(width / base.width);
      setFitScale(prev => {
        // Keep the reader's page in place when the panel is resized, but not
        // for the first fit (the passage locator positions the document).
        if (fitInitialized.current && zoomModeRef.current === 'fit' && Math.abs(next - prev) > 0.001) {
          anchorPage.current = currentPageRef.current;
        }
        return next;
      });
      fitInitialized.current = true;
    };
    update();
    const observer = new ResizeObserver(update);
    observer.observe(scroller);
    return () => observer.disconnect();
  }, [sizes.length > 0 ? sizes[0].width : 0]);

  // Lazy rendering: pages within one screen of the viewport are active.
  useEffect(() => {
    const scroller = scrollerRef.current;
    if (!scroller || !doc) return;
    const observer = new IntersectionObserver(
      entries => {
        setVisible(prev => {
          const next = new Set(prev);
          for (const entry of entries) {
            const page = Number((entry.target as HTMLElement).dataset.page);
            if (entry.isIntersecting) next.add(page);
            else next.delete(page);
          }
          return next;
        });
      },
      { root: scroller, rootMargin: '100% 0px' },
    );
    observerRef.current = observer;
    pageEls.current.forEach(el => observer.observe(el));
    return () => {
      observer.disconnect();
      observerRef.current = null;
    };
  }, [doc]);

  const registerElement = useCallback((page: number, el: HTMLDivElement | null) => {
    const previous = pageEls.current.get(page);
    if (previous && previous !== el) observerRef.current?.unobserve(previous);
    if (el) {
      pageEls.current.set(page, el);
      observerRef.current?.observe(el);
    } else {
      pageEls.current.delete(page);
    }
  }, []);

  const scrollToPage = useCallback((page: number, behavior: ScrollBehavior = 'auto') => {
    const scroller = scrollerRef.current;
    const el = pageEls.current.get(page);
    if (!scroller || !el) return;
    scroller.scrollTo({ top: Math.max(0, el.offsetTop - PAGE_GAP), behavior });
  }, []);

  // Keep the page the reader was on in place across zoom changes.
  useLayoutEffect(() => {
    if (anchorPage.current !== null) {
      scrollToPage(anchorPage.current);
      anchorPage.current = null;
    }
  }, [scale, scrollToPage]);

  // Track the page at the top third of the viewport.
  useEffect(() => {
    const scroller = scrollerRef.current;
    if (!scroller || !doc) return;
    let frame = 0;
    const onScroll = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const probe = scroller.scrollTop + scroller.clientHeight / 3;
        let lo = 1;
        let hi = doc.numPages;
        while (lo < hi) {
          const mid = Math.ceil((lo + hi) / 2);
          const el = pageEls.current.get(mid);
          if (el && el.offsetTop <= probe) lo = mid;
          else hi = mid - 1;
        }
        setCurrentPage(lo);
      });
    };
    scroller.addEventListener('scroll', onScroll, { passive: true });
    return () => {
      cancelAnimationFrame(frame);
      scroller.removeEventListener('scroll', onScroll);
    };
  }, [doc]);

  const citedStart = citedPages?.start ?? null;
  const citedEnd = citedPages?.end ?? null;

  // Locate the passage: cited page(s) first, then every page by distance.
  useEffect(() => {
    if (!doc) return;
    const token = ++locateToken.current;
    const total = doc.numPages;
    const isCurrent = () => token === locateToken.current;
    setTarget(null);
    scrolledToken.current = null;

    const start = citedStart !== null ? Math.min(Math.max(1, citedStart), total) : null;
    const end = start !== null && citedEnd !== null ? Math.min(Math.max(start, citedEnd), total) : start;
    const citedList: number[] = [];
    if (start !== null && end !== null) for (let p = start; p <= end; p += 1) citedList.push(p);
    requestAnimationFrame(() => {
      if (isCurrent()) scrollToPage(start ?? 1);
    });

    const passageText = passage.trim();
    if (!passageText) {
      onLocateRef.current({
        status: 'notFound',
        message: start !== null ? `No passage text was stored for this source; showing page ${start}.` : 'No passage text was stored for this source; showing the document from the start.',
      });
      return;
    }

    const search = async (pages: number[], onProgress?: (done: number) => void) => {
      let best: { page: number; match: PassageMatch } | null = null;
      for (let i = 0; i < pages.length; i += 1) {
        if (!isCurrent()) return null;
        const text = await getText(pages[i]);
        const match = findPassage(passageText, text.haystack);
        if (match && (!best || matchScore(match) > matchScore(best.match))) best = { page: pages[i], match };
        if (best && best.match.coverage >= CONFIDENT_COVERAGE && !best.match.partial) break;
        if (onProgress && i % 10 === 9) onProgress(i + 1);
      }
      return best;
    };

    (async () => {
      onLocateRef.current({ status: 'searching', message: start !== null ? `Finding the passage on page ${start}…` : 'Finding the passage in this PDF…' });
      let best = citedList.length > 0 ? await search(citedList) : null;
      if (!isCurrent()) return;
      if (!best) {
        const anchor = start ?? 1;
        const rest: number[] = [];
        for (let p = 1; p <= total; p += 1) if (!citedList.includes(p)) rest.push(p);
        rest.sort((a, b) => Math.abs(a - anchor) - Math.abs(b - anchor) || a - b);
        best = await search(rest, done => {
          if (isCurrent()) onLocateRef.current({ status: 'searching', message: `Searching the PDF for the passage… (${done} of ${rest.length} pages)` });
        });
        if (!isCurrent()) return;
      }

      if (!best) {
        onLocateRef.current({
          status: 'notFound',
          message:
            start !== null
              ? `The cited passage could not be pinpointed in the PDF text; showing page ${start}.`
              : 'The cited passage could not be found in the PDF text; showing the document from the start.',
        });
        return;
      }

      setTarget({ page: best.page, ranges: best.match.ranges, token });
      scrollToPage(best.page);
      const moved = start !== null && (best.page < start || best.page > (end ?? start));
      const where = `page ${best.page}`;
      if (best.match.partial || best.match.coverage < 0.6) {
        onLocateRef.current({
          status: 'approximate',
          message: `Highlighted the closest match on ${where}; the retrieved text differs from the PDF text in places.`,
        });
      } else {
        onLocateRef.current({
          status: 'found',
          message: moved ? `Cited passage highlighted on ${where} (indexed as page ${start}).` : `Cited passage highlighted on ${where}.`,
        });
      }
    })().catch(() => {
      if (!isCurrent()) return;
      onLocateRef.current({
        status: 'notFound',
        message: start !== null ? `The PDF text could not be searched; showing page ${start}.` : 'The PDF text could not be searched; showing the document from the start.',
      });
    });
  }, [doc, passage, citedStart, citedEnd, getText, scrollToPage]);

  const onHighlightRendered = useCallback((page: number, token: number, top: number | null) => {
    if (scrolledToken.current === token || token !== locateToken.current) return;
    const scroller = scrollerRef.current;
    const el = pageEls.current.get(page);
    if (!scroller || !el) return;
    scrolledToken.current = token;
    if (top === null) {
      // The passage matched the page text but could not be outlined on the
      // rendered page (text layer unavailable or out of step with it).
      onLocateRef.current({
        status: 'approximate',
        message: `The cited passage is on page ${page}, but it could not be outlined on the rendered page.`,
      });
      return;
    }
    scroller.scrollTo({ top: Math.max(0, el.offsetTop + top - scroller.clientHeight / 3), behavior: scrollBehavior() });
  }, []);

  const zoomBy = (factor: number) => {
    anchorPage.current = currentPage;
    setZoom({ mode: 'manual', scale: clampScale(scale * factor) });
  };
  const fitWidth = () => {
    anchorPage.current = currentPage;
    setZoom({ mode: 'fit' });
  };
  const goToPage = (page: number) => {
    if (!numPages) return;
    const clamped = Math.min(Math.max(1, page), numPages);
    setCurrentPage(clamped);
    scrollToPage(clamped, scrollBehavior());
  };

  const toolButton = cn(
    'w-8 h-8 inline-flex items-center justify-center rounded-lg text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text disabled:opacity-40 disabled:cursor-not-allowed transition-colors duration-micro',
    VIEWER_FOCUS_RING,
  );

  return (
    <div className="flex-1 min-h-0 flex flex-col">
      <div role="toolbar" aria-label="PDF controls" className="flex items-center gap-1 px-3 py-1.5 border-b border-shodh-border-subtle text-[12.5px] text-shodh-text-secondary">
        <button type="button" className={toolButton} onClick={() => goToPage(currentPage - 1)} disabled={!doc || currentPage <= 1} aria-label="Previous page">
          <ChevronUp className="w-4 h-4" aria-hidden="true" />
        </button>
        <button type="button" className={toolButton} onClick={() => goToPage(currentPage + 1)} disabled={!doc || currentPage >= numPages} aria-label="Next page">
          <ChevronDown className="w-4 h-4" aria-hidden="true" />
        </button>
        <form
          className="flex items-center gap-1.5 ml-1"
          onSubmit={e => {
            e.preventDefault();
            const n = Number(pageInput);
            if (Number.isInteger(n)) goToPage(n);
            else setPageInput(String(currentPage));
          }}
        >
          <label htmlFor="pdf-page-input" className="sr-only">
            Page number
          </label>
          <span aria-hidden="true">Page</span>
          <input
            id="pdf-page-input"
            inputMode="numeric"
            value={pageInput}
            onChange={e => setPageInput(e.target.value.replace(/[^0-9]/g, ''))}
            onBlur={() => setPageInput(String(currentPage))}
            disabled={!doc}
            className={cn('w-12 h-7 px-1.5 rounded-md border border-shodh-border-strong bg-shodh-surface-2 text-center tabular-nums text-shodh-text', VIEWER_FOCUS_RING)}
          />
          <span className="tabular-nums">of {numPages || '–'}</span>
        </form>
        <div className="flex-1" />
        <button type="button" className={toolButton} onClick={() => zoomBy(1 / ZOOM_STEP)} disabled={!doc || scale <= MIN_SCALE} aria-label="Zoom out">
          <Minus className="w-4 h-4" aria-hidden="true" />
        </button>
        <span className="w-12 text-center tabular-nums" aria-label={`Zoom ${Math.round(scale * 100)} percent`}>
          {Math.round(scale * 100)}%
        </span>
        <button type="button" className={toolButton} onClick={() => zoomBy(ZOOM_STEP)} disabled={!doc || scale >= MAX_SCALE} aria-label="Zoom in">
          <Plus className="w-4 h-4" aria-hidden="true" />
        </button>
        <button
          type="button"
          className={cn(toolButton, 'w-auto px-2 gap-1.5', zoom.mode === 'fit' && 'bg-shodh-raised text-shodh-text')}
          onClick={fitWidth}
          disabled={!doc}
          aria-pressed={zoom.mode === 'fit'}
        >
          <MoveHorizontal className="w-4 h-4" aria-hidden="true" />
          Fit width
        </button>
      </div>

      <div
        ref={scrollerRef}
        tabIndex={0}
        aria-label="PDF pages"
        className={cn('relative flex-1 min-h-0 overflow-auto scrollbar-thin bg-shodh-raised-2', VIEWER_FOCUS_RING)}
        style={{ padding: PAGE_PADDING }}
      >
        {!doc ? (
          <div className="h-full flex items-center justify-center gap-2 text-[13px] text-shodh-text-muted">
            <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
            Loading PDF…
          </div>
        ) : (
          <div className="flex flex-col items-center" style={{ gap: PAGE_GAP }}>
            {sizes.map((size, index) => {
              const pageNumber = index + 1;
              const isTarget = target?.page === pageNumber;
              return (
                <PdfPageView
                  key={pageNumber}
                  doc={doc}
                  pageNumber={pageNumber}
                  scale={scale}
                  size={{ width: Math.floor(size.width * scale), height: Math.floor(size.height * scale) }}
                  active={visible.has(pageNumber) || isTarget}
                  getText={getText}
                  ranges={isTarget ? target.ranges : null}
                  highlightToken={isTarget ? target.token : null}
                  onHighlightRendered={onHighlightRendered}
                  registerElement={registerElement}
                />
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
