/**
 * The Reader: any indexed file, edge to edge. PDF pages are rendered on the
 * computer at this phone's width, page by page as they scroll in; an image
 * fills the screen and pinches to zoom; text, Markdown, code and Office
 * documents are their extracted text, reflowed; spreadsheets are their
 * sheets. Opened from a citation, it starts at the cited page with the
 * cited passage shown first. It zooms open from the page that was tapped.
 */
import { useEffect, useRef, useState, type PointerEvent } from 'react';
import { motion } from 'framer-motion';
import { ChevronLeft, LoaderCircle } from 'lucide-react';
import { NotConnected } from '../components/Connection';
import { useLink, useNav } from '../lib/context';
import { fileInfo, readTable, readText, type FileInfo, type Sheet, type TextPart } from '../lib/desktop';
import { errorText } from '../lib/link';
import { glide } from '../lib/motion';
import type { Origin, ReaderTarget } from '../lib/nav';
import { pageImage, readerWidth, type Page } from '../lib/pages';
import { useRemote } from '../lib/remote';

/** The Reader's opening: from the tapped page's place and size to the whole screen. */
function zoomFrom(origin: Origin | null) {
  if (!origin || typeof window === 'undefined' || window.innerWidth === 0) return { opacity: 0, scale: 0.92, x: 0, y: 0 };
  const scale = Math.max(0.1, Math.min(1, origin.width / window.innerWidth));
  const x = origin.x + origin.width / 2 - window.innerWidth / 2;
  const y = origin.y + origin.height / 2 - window.innerHeight / 2;
  return { opacity: 0, scale, x, y };
}

export function Reader({ target }: { target: ReaderTarget }) {
  const { connected } = useLink();
  const { back } = useNav();
  const info = useRemote(`file:${target.path}`, desktop => fileInfo(desktop, target.path));
  const [pages, setPages] = useState<number | null>(null);
  const start = zoomFrom(target.origin);

  const where =
    info.data?.kind === 'pdf' && pages !== null
      ? `${target.page ? `Page ${target.page} of ${pages}` : `${pages} ${pages === 1 ? 'page' : 'pages'}`}`
      : (info.data?.fileName ?? target.name);

  return (
    <motion.div
      className="reader"
      role="dialog"
      aria-label={target.name}
      initial={{ opacity: start.opacity, scale: start.scale, x: start.x, y: start.y }}
      animate={{ opacity: 1, scale: 1, x: 0, y: 0 }}
      exit={{ opacity: 0, scale: 0.96 }}
      transition={glide}
    >
      <div className="reader-bar">
        <button type="button" className="back" onClick={back}>
          <ChevronLeft aria-hidden="true" className="icon" />
          <span>{target.back}</span>
        </button>
        <span className="reader-where">{where}</span>
      </div>
      <div className="reader-body">
        {target.passage && (
          <p className="passage">
            <span className="passage-label">{target.page ? `Cited passage, page ${target.page}` : 'Cited passage'}</span>
            <mark className="passage-mark">{target.passage}</mark>
          </p>
        )}
        {info.data === undefined && !connected && <NotConnected what="this file" />}
        {info.data === undefined && connected && info.error === null && <PageSkeleton />}
        {info.error !== null && info.data === undefined && <p className="panel-text">{info.error}</p>}
        {info.data && <Content info={info.data} target={target} onPages={setPages} />}
      </div>
    </motion.div>
  );
}

function PageSkeleton() {
  return (
    <div className="paper reader-page" aria-busy="true" aria-label="Loading">
      <span className="paper-skeleton" />
    </div>
  );
}

function Content({ info, target, onPages }: { info: FileInfo; target: ReaderTarget; onPages: (pages: number) => void }) {
  switch (info.kind) {
    case 'pdf':
      return <Pages path={info.path} first={target.page ?? 1} onPages={onPages} />;
    case 'image':
      return <ZoomableImage path={info.path} name={info.fileName} />;
    case 'text':
      return <Text path={info.path} extension={info.extension} />;
    case 'table':
      return <Sheets path={info.path} />;
    case 'unsupported':
      return <p className="panel-text">This kind of file cannot be shown on the phone. Open it in Shodh on your computer.</p>;
  }
}

/** A PDF's pages: each asked for when it scrolls near, the cited page first in view. */
function Pages({ path, first, onPages }: { path: string; first: number; onPages: (pages: number) => void }) {
  const { desktop } = useLink();
  const [count, setCount] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const width = readerWidth();

  // The first page shown says how many there are.
  useEffect(() => {
    let alive = true;
    pageImage(desktop, path, first, width)
      .then(page => {
        if (!alive) return;
        setCount(page.pages);
        onPages(page.pages);
      })
      .catch(e => {
        if (alive) setError(errorText(e));
      });
    return () => {
      alive = false;
    };
  }, [desktop, path, first, width, onPages]);

  if (error) return <p className="panel-text">{error}</p>;
  if (count === null) return <PageSkeleton />;
  const numbers = Array.from({ length: count }, (_, i) => i + 1);
  return (
    <>
      {numbers.map(n => (
        <PageView key={n} path={path} page={n} width={width} scrollTo={n === first && first > 1} />
      ))}
    </>
  );
}

function PageView({ path, page, width, scrollTo }: { path: string; page: number; width: number; scrollTo: boolean }) {
  const { desktop } = useLink();
  const ref = useRef<HTMLDivElement>(null);
  const [near, setNear] = useState(scrollTo);
  const [image, setImage] = useState<Page | null>(null);
  const [failed, setFailed] = useState<string | null>(null);

  useEffect(() => {
    if (scrollTo) ref.current?.scrollIntoView({ block: 'start' });
  }, [scrollTo]);

  useEffect(() => {
    const element = ref.current;
    if (!element || near) return;
    if (typeof IntersectionObserver === 'undefined') {
      setNear(true);
      return;
    }
    const observer = new IntersectionObserver(entries => {
      if (entries.some(e => e.isIntersecting)) {
        setNear(true);
        observer.disconnect();
      }
    }, { rootMargin: '600px' });
    observer.observe(element);
    return () => observer.disconnect();
  }, [near]);

  useEffect(() => {
    if (!near) return;
    let alive = true;
    pageImage(desktop, path, page, width)
      .then(loaded => {
        if (alive) setImage(loaded);
      })
      .catch(e => {
        if (alive) setFailed(errorText(e));
      });
    return () => {
      alive = false;
    };
  }, [desktop, path, page, width, near]);

  return (
    <div ref={ref} className="paper reader-page" style={image ? undefined : { aspectRatio: '0.77' }}>
      {image ? (
        <img src={image.src} alt={`Page ${page}`} width={image.width} height={image.height} />
      ) : failed ? (
        <p className="paper-type-name" style={{ padding: 20 }}>{`Page ${page} could not be shown: ${failed}`}</p>
      ) : (
        <span className="paper-skeleton" aria-hidden="true" />
      )}
    </div>
  );
}

const MAX_ZOOM = 5;

/** An image, full width, pinched to zoom and dragged when zoomed; a double tap zooms in and out. */
function ZoomableImage({ path, name }: { path: string; name: string }) {
  const { desktop } = useLink();
  const [image, setImage] = useState<Page | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [view, setView] = useState({ scale: 1, x: 0, y: 0 });
  const pointers = useRef(new Map<number, { x: number; y: number }>());
  const pinch = useRef<{ distance: number; scale: number } | null>(null);
  const lastTap = useRef(0);

  useEffect(() => {
    let alive = true;
    pageImage(desktop, path, 1, readerWidth())
      .then(loaded => {
        if (alive) setImage(loaded);
      })
      .catch(e => {
        if (alive) setError(errorText(e));
      });
    return () => {
      alive = false;
    };
  }, [desktop, path]);

  const distance = () => {
    const [a, b] = [...pointers.current.values()];
    return a && b ? Math.hypot(a.x - b.x, a.y - b.y) : 0;
  };
  const down = (event: PointerEvent<HTMLDivElement>) => {
    event.currentTarget.setPointerCapture?.(event.pointerId);
    pointers.current.set(event.pointerId, { x: event.clientX, y: event.clientY });
    if (pointers.current.size === 2) pinch.current = { distance: distance(), scale: view.scale };
    if (pointers.current.size === 1) {
      const now = Date.now();
      if (now - lastTap.current < 300) setView(v => (v.scale > 1 ? { scale: 1, x: 0, y: 0 } : { scale: 2.5, x: 0, y: 0 }));
      lastTap.current = now;
    }
  };
  const move = (event: PointerEvent<HTMLDivElement>) => {
    const before = pointers.current.get(event.pointerId);
    if (!before) return;
    pointers.current.set(event.pointerId, { x: event.clientX, y: event.clientY });
    if (pointers.current.size === 2 && pinch.current && pinch.current.distance > 0) {
      const scale = Math.min(MAX_ZOOM, Math.max(1, (pinch.current.scale * distance()) / pinch.current.distance));
      setView(v => ({ ...v, scale }));
    } else if (pointers.current.size === 1 && view.scale > 1) {
      setView(v => ({ ...v, x: v.x + event.clientX - before.x, y: v.y + event.clientY - before.y }));
    }
  };
  const up = (event: PointerEvent<HTMLDivElement>) => {
    pointers.current.delete(event.pointerId);
    if (pointers.current.size < 2) pinch.current = null;
    if (view.scale <= 1) setView({ scale: 1, x: 0, y: 0 });
  };

  if (error) return <p className="panel-text">{error}</p>;
  if (!image) return <PageSkeleton />;
  return (
    <div className="paper" style={{ overflow: 'hidden' }} onPointerDown={down} onPointerMove={move} onPointerUp={up} onPointerCancel={up}>
      <img
        className="zoomable"
        src={image.src}
        alt={name}
        width={image.width}
        height={image.height}
        style={{ transform: `translate(${view.x}px, ${view.y}px) scale(${view.scale})` }}
        draggable={false}
      />
    </div>
  );
}

const CODE = /\.(rs|ts|tsx|js|jsx|py|go|java|kt|swift|c|cc|cpp|h|hpp|cs|rb|php|sh|ps1|sql|toml|yaml|yml|json|css|scss|html|htm|xml|ini|cfg|conf|env|log)$/i;

/** Extracted text, in parts; more is read when asked. */
function Text({ path, extension }: { path: string; extension: string }) {
  const { desktop, connected } = useLink();
  const [parts, setParts] = useState<TextPart[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const read = (part: number) => {
    setLoading(true);
    setError(null);
    readText(desktop, path, part)
      .then(next => setParts(all => (all.some(p => p.part === next.part) ? all : [...all, next])))
      .catch(e => setError(errorText(e)))
      .finally(() => setLoading(false));
  };
  useEffect(() => {
    // The first part, once the computer can be asked; later parts when asked for.
    if (connected && parts.length === 0) read(0);
  }, [path, connected]);

  const last = parts[parts.length - 1];
  const code = CODE.test(`.${extension}`);
  if (error && parts.length === 0) return <p className="panel-text">{error}</p>;
  if (parts.length === 0) return <PageSkeleton />;
  return (
    <>
      <div className={`reader-text${code ? ' reader-code' : ''}`}>{parts.map(p => p.text).join('')}</div>
      {last && last.part + 1 < last.parts && (
        <button type="button" className="button button-wide" disabled={loading} onClick={() => read(last.part + 1)}>
          {loading && <LoaderCircle aria-hidden="true" className="icon-small spin" />}
          {`Show more (part ${last.part + 2} of ${last.parts})`}
        </button>
      )}
      {last?.truncated && <p className="panel-text">This file is long; your computer shows the rest.</p>}
      {error && <p className="panel-text">{error}</p>}
    </>
  );
}

function Sheets({ path }: { path: string }) {
  const sheets = useRemote<Sheet[]>(`sheets:${path}`, desktop => readTable(desktop, path));
  if (sheets.error) return <p className="panel-text">{sheets.error}</p>;
  if (!sheets.data) return <PageSkeleton />;
  return (
    <>
      {sheets.data.map(sheet => (
        <section key={sheet.name} className="sheet-table" aria-label={sheet.name}>
          <p className="passage-label">{sheet.truncated ? `${sheet.name}: the first ${sheet.rows.length} of ${sheet.totalRows} rows` : sheet.name}</p>
          <table>
            <thead>
              <tr>
                {sheet.headers.map((h, i) => (
                  <th key={i}>{h}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {sheet.rows.map((row, r) => (
                <tr key={r}>
                  {row.map((cell, c) => (
                    <td key={c}>{cell}</td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      ))}
    </>
  );
}
