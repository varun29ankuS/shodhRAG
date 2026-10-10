import React, { Suspense, lazy } from 'react';
import type { PdfViewer as PdfViewerImpl } from './PdfViewer';

// The PDF viewer (and the pdf.js glue under it) loads with the first PDF
// opened, not at startup.
const PdfViewerView = lazy(() => import('./PdfViewer').then(m => ({ default: m.PdfViewer })));

type PdfViewerProps = React.ComponentProps<typeof PdfViewerImpl>;

/** The viewer's area, empty, while its code loads. */
function PdfViewerLoading() {
  return (
    <div className="flex-1 min-h-0 flex items-center justify-center text-[13px] text-shodh-text-muted" role="status" aria-busy="true">
      Opening…
    </div>
  );
}

export function PdfViewer(props: PdfViewerProps) {
  return (
    <Suspense fallback={<PdfViewerLoading />}>
      <PdfViewerView {...props} />
    </Suspense>
  );
}
