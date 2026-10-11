import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { FileText, Loader2 } from 'lucide-react';
import { cn } from '../../../lib/utils';
import { FocusFrame } from '../../focus/FocusFrame';
import { figureTarget } from '../../focus/targets';
import { FigureImage } from '../../research/FigureImage';
import { figureRegion, parseFigureBlock, resolveFigure } from '../../research/paperObjects';
import type { PaperParts } from '../../research/paperObjects';
import { figureError, paperParts } from '../../research/paperObjectsApi';
import { showSourceBox } from '../../research/snippetBus';
import { BlockError } from './VisualBlocks';

type Paper = { status: 'loading' } | { status: 'ready'; parts: PaperParts } | { status: 'error'; message: string };

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

/**
 * A ```figure block: the paper's own figure, cropped from its PDF page at
 * high resolution, with its caption, "Show in PDF" and Expand & ask. The
 * paper must be an indexed PDF (checked by the backend before anything is
 * read); the crop uses the path the backend confirmed, never the block's.
 */
export function FigureBlock({ source }: { source: string }) {
  const parsed = useMemo(() => parseFigureBlock(source), [source]);
  const spec = 'spec' in parsed ? parsed.spec : null;
  const paper = spec ? spec.paper : null;
  const [state, setState] = useState<Paper>({ status: 'loading' });

  useEffect(() => {
    if (!paper) return;
    let cancelled = false;
    setState({ status: 'loading' });
    paperParts(paper)
      .then(parts => {
        if (!cancelled) setState({ status: 'ready', parts });
      })
      .catch(error => {
        if (!cancelled) setState({ status: 'error', message: figureError(error) });
      });
    return () => {
      cancelled = true;
    };
  }, [paper]);

  const parts = state.status === 'ready' ? state.parts : null;
  const figure = spec && parts ? resolveFigure(spec, parts.figures) : null;

  const getTarget = useCallback(() => {
    if (!parts || !figure) return null;
    return figureTarget({
      filePath: parts.filePath,
      fileName: parts.fileName,
      page: figure.page,
      bbox: figure.bbox,
      figureId: figure.figureId,
      caption: figure.caption,
      label: figure.label,
      mentions: figure.mentions,
    });
  }, [parts, figure]);

  if ('error' in parsed) return <BlockError title="Figure not shown" message={parsed.error} source={source} />;
  if (state.status === 'error') return <BlockError title="Figure not shown" message={state.message} source={source} />;
  if (state.status === 'loading' || !parts) {
    return (
      <figure className="my-4 rounded-xl border border-shodh-border bg-shodh-surface" aria-busy="true">
        <div role="status" className="flex items-center gap-2 h-24 justify-center text-[12.5px] text-shodh-text-muted">
          <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
          Finding the figure in the paper…
        </div>
      </figure>
    );
  }
  if (!figure) {
    const known = parts.figures.map(f => f.label).join(', ');
    return (
      <BlockError
        title="Figure not shown"
        message={`${parts.fileName} has no figure ${spec?.figureId ?? ''}${known ? ` (it has: ${known})` : ''}.`}
        source={source}
      />
    );
  }

  const caption = figure.caption || (spec?.caption ?? '');
  const showInPdf = () =>
    showSourceBox({
      filePath: parts.filePath,
      fileName: parts.fileName,
      page: figure.page,
      regions: [figureRegion(figure.page, figure.bbox)],
      label: `${figure.label} · ${parts.fileName}`,
    });

  return (
    <FocusFrame noun="figure" getTarget={getTarget} className="my-4">
      <figure className="m-0 rounded-xl border border-shodh-border bg-shodh-surface overflow-hidden">
        {/* Papers are printed on white: keep the figure on white in both themes. */}
        <div className="bg-white p-3 flex justify-center">
          <FigureImage filePath={parts.filePath} page={figure.page} bbox={figure.bbox} alt={caption || `${figure.label} of ${parts.fileName}`} className="max-h-[560px] w-auto" />
        </div>
        <figcaption className="flex items-start gap-3 px-3 py-2 border-t border-shodh-border-subtle text-[13px] leading-snug text-shodh-text-secondary">
          <span className="flex-1 min-w-0">
            {caption || figure.label}
            <span className="block mt-0.5 text-[11.5px] text-shodh-text-muted">{`${parts.fileName}, page ${figure.page}`}</span>
          </span>
          <button
            type="button"
            onClick={showInPdf}
            className={cn(
              'shrink-0 h-7 px-2 inline-flex items-center gap-1.5 rounded-lg text-[11.5px] text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
              FOCUS_RING,
            )}
            aria-label={`Show ${figure.label} in ${parts.fileName}, page ${figure.page}`}
          >
            <FileText className="w-3.5 h-3.5" aria-hidden="true" />
            Show in PDF
          </button>
        </figcaption>
      </figure>
    </FocusFrame>
  );
}
