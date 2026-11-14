/**
 * A file as a small page: the real page rendered on the computer for a PDF
 * or an image (`read_source_page`), loaded when it scrolls into view, or
 * the file's name and type on paper for a file without pages.
 */
import { useEffect, useRef, useState } from 'react';
import { useLink } from '../lib/context';
import { extensionOf, hasPages, pageImage, THUMB_WIDTH, type Page } from '../lib/pages';
import { fileName } from '../lib/answer';

interface Props {
  path: string;
  /** 1-based page; the first when not known. */
  page?: number | null;
  /** The citation number shown on the page, for a source. */
  badge?: number | null;
  className?: string;
}

export function PageThumb({ path, page = null, badge = null, className = '' }: Props) {
  const { desktop, connected } = useLink();
  const ref = useRef<HTMLSpanElement>(null);
  const [visible, setVisible] = useState(false);
  const [image, setImage] = useState<Page | null>(null);
  const [failed, setFailed] = useState(false);
  const paged = hasPages(path);

  // Only pages that scroll into view are asked for.
  useEffect(() => {
    const element = ref.current;
    if (!element || !paged) return;
    if (typeof IntersectionObserver === 'undefined') {
      setVisible(true);
      return;
    }
    const observer = new IntersectionObserver(
      entries => {
        if (entries.some(e => e.isIntersecting)) {
          setVisible(true);
          observer.disconnect();
        }
      },
      { rootMargin: '200px' },
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, [paged]);

  useEffect(() => {
    if (!paged || !visible || !connected) return;
    let alive = true;
    setFailed(false);
    pageImage(desktop, path, page ?? 1, THUMB_WIDTH)
      .then(loaded => {
        if (alive) setImage(loaded);
      })
      .catch(() => {
        if (alive) setFailed(true);
      });
    return () => {
      alive = false;
    };
  }, [desktop, connected, path, page, paged, visible]);

  const name = fileName(path);
  return (
    <span ref={ref} className={`paper paper-thumb ${className}`}>
      {image ? (
        <img src={image.src} alt={`${name}, page ${page ?? 1}`} />
      ) : paged && !failed ? (
        <span className="paper-skeleton" aria-hidden="true" />
      ) : (
        <span className="paper-type">
          <span className="paper-type-name">{name}</span>
          <span className="paper-type-ext">{extensionOf(path) || 'file'}</span>
        </span>
      )}
      {badge !== null && <span className="paper-badge">{badge}</span>}
    </span>
  );
}
