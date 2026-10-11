import React, { useEffect, useState } from 'react';
import { AlertTriangle, Loader2 } from 'lucide-react';
import { cn } from '../../lib/utils';
import { figureError, figureImage } from './paperObjectsApi';
import type { FigureImage as Image } from './paperObjectsApi';
import type { PdfBoxCorners } from './paperObjects';

type State = { status: 'loading' } | { status: 'ready'; image: Image } | { status: 'error'; message: string };

export interface FigureImageProps {
  /** An indexed PDF (as returned by `paper_objects`). */
  filePath: string;
  page: number;
  bbox: PdfBoxCorners;
  alt: string;
  size?: 'full' | 'thumb';
  className?: string;
  /** The image's natural size once drawn (the pop-out fits and zooms it). */
  onReady?: (image: Image) => void;
}

/** A figure cropped from its PDF page with pdf.js (never a screenshot). */
export function FigureImage({ filePath, page, bbox, alt, size = 'full', className, onReady }: FigureImageProps) {
  const [state, setState] = useState<State>({ status: 'loading' });
  const { x0, y0, x1, y1 } = bbox;

  useEffect(() => {
    let cancelled = false;
    setState({ status: 'loading' });
    figureImage(filePath, page, { x0, y0, x1, y1 }, size)
      .then(image => {
        if (cancelled) return;
        setState({ status: 'ready', image });
        onReady?.(image);
      })
      .catch(error => {
        if (!cancelled) setState({ status: 'error', message: figureError(error) });
      });
    return () => {
      cancelled = true;
    };
    // onReady is a notification; a new function must not redraw the figure.
  }, [filePath, page, x0, y0, x1, y1, size]);

  if (state.status === 'error') {
    return (
      <p role="alert" className={cn('flex items-start gap-2 p-3 text-[12.5px] text-shodh-text-secondary', className)}>
        <AlertTriangle className="w-3.5 h-3.5 mt-0.5 shrink-0 text-shodh-warning" aria-hidden="true" />
        {`The figure could not be drawn: ${state.message}`}
      </p>
    );
  }
  if (state.status === 'loading') {
    const ratio = Math.min(1.2, Math.max(0.2, (y1 - y0) / Math.max(1, x1 - x0)));
    return (
      <div
        role="status"
        aria-label="Drawing the figure from the PDF"
        className={cn('flex items-center justify-center w-full text-shodh-text-muted', className)}
        style={{ aspectRatio: `${1 / ratio}` }}
      >
        <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
      </div>
    );
  }
  return (
    <img
      src={state.image.url}
      alt={alt}
      width={state.image.width}
      height={state.image.height}
      draggable={false}
      className={cn('block max-w-full h-auto', className)}
    />
  );
}
