import React, { useEffect, useRef, useState } from 'react';
import { Loader2 } from 'lucide-react';
import { cn } from '../../../lib/utils';
import { readSourceBytes } from './sourceAccess';
import type { LocateResult } from './viewerTypes';
import { VIEWER_FOCUS_RING } from './viewerTypes';

interface ImageViewerProps {
  filePath: string;
  fileName: string;
  mimeType: string;
  passage: string;
  onLocate: (result: LocateResult) => void;
  onError: (error: unknown) => void;
}

/** The indexed image itself, with the text recognised in it (OCR) below. */
export function ImageViewer({ filePath, fileName, mimeType, passage, onLocate, onError }: ImageViewerProps) {
  const [url, setUrl] = useState<string | null>(null);
  const [broken, setBroken] = useState(false);

  const onLocateRef = useRef(onLocate);
  const onErrorRef = useRef(onError);
  useEffect(() => {
    onLocateRef.current = onLocate;
    onErrorRef.current = onError;
  }, [onLocate, onError]);

  useEffect(() => {
    let cancelled = false;
    let objectUrl: string | null = null;
    setUrl(null);
    setBroken(false);
    readSourceBytes(filePath)
      .then(bytes => {
        if (cancelled) return;
        objectUrl = URL.createObjectURL(new Blob([bytes], { type: mimeType }));
        setUrl(objectUrl);
      })
      .catch(error => {
        if (!cancelled) onErrorRef.current(error);
      });
    return () => {
      cancelled = true;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [filePath, mimeType]);

  useEffect(() => {
    onLocateRef.current(
      passage.trim()
        ? { status: 'found', message: 'The cited text was recognised in this image; it is listed below the image.' }
        : { status: 'notFound', message: 'No recognised text was stored for this image.' },
    );
  }, [passage]);

  return (
    <div tabIndex={0} aria-label="Image and recognised text" className={cn('flex-1 min-h-0 overflow-y-auto scrollbar-thin p-5 flex flex-col gap-5', VIEWER_FOCUS_RING)}>
      <div className="rounded-xl border border-shodh-border bg-shodh-surface-2 p-3 flex items-center justify-center min-h-[200px]">
        {broken ? (
          <p className="text-[13px] text-shodh-text-muted">This image could not be displayed.</p>
        ) : url ? (
          <img src={url} alt={fileName} className="max-w-full h-auto rounded-md" onError={() => setBroken(true)} />
        ) : (
          <span className="inline-flex items-center gap-2 text-[13px] text-shodh-text-muted">
            <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
            Loading image…
          </span>
        )}
      </div>
      {passage.trim() && (
        <figure className="m-0 flex flex-col gap-2">
          <figcaption className="text-[11.5px] font-medium uppercase tracking-wider text-shodh-text-faint">
            Recognised text (cited passage)
          </figcaption>
          <blockquote className="m-0 rounded-xl border border-shodh-border bg-shodh-surface-2 px-4 py-3.5 text-[14px] leading-[1.7] text-shodh-text-secondary whitespace-pre-wrap break-words">
            <mark className="source-mark">{passage.trim()}</mark>
          </blockquote>
        </figure>
      )}
    </div>
  );
}
