import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { readSourceBytes } from '../features/ask/viewer/sourceAccess';
import { FileText, AlertCircle } from 'lucide-react';

interface PDFArtifactProps {
  artifact: {
    id: string;
    title: string;
    content: string; // This will be the file path
    metadata?: {
      filePath?: string;
      pageNumber?: number;
      snippet?: string;
      lineRange?: [number, number];
    };
  };
}

export const PDFArtifact: React.FC<PDFArtifactProps> = ({ artifact }) => {
  const [pdfUrl, setPdfUrl] = useState<string>('');
  const [error, setError] = useState<string>('');
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    const loadPdf = async () => {
      try {
        setLoading(true);
        setError('');

        // Get file path from metadata or content
        const filePath = artifact.metadata?.filePath || artifact.content;

        if (!filePath) {
          throw new Error('No file path provided');
        }

        console.log('Loading PDF from path:', filePath);

        // Indexed documents only: the backend refuses any other path.
        const fileData = await readSourceBytes(filePath);
        console.log('Read PDF file, size:', fileData.length, 'bytes');

        // Convert to blob URL
        const blob = new Blob([fileData], { type: 'application/pdf' });
        let blobUrl = URL.createObjectURL(blob);
        console.log('Created blob URL:', blobUrl);

        // Add PDF fragment parameters to navigate to page and highlight text
        const fragments: string[] = [];

        // Navigate to specific page if available
        if (artifact.metadata?.pageNumber) {
          fragments.push(`page=${artifact.metadata.pageNumber}`);
          console.log(`📍 Navigating to page ${artifact.metadata.pageNumber}`);
        }

        // Add fragments to URL
        if (fragments.length > 0) {
          blobUrl += '#' + fragments.join('&');
          console.log('📄 PDF URL with navigation:', blobUrl);
        }

        setPdfUrl(blobUrl);
        setLoading(false);
      } catch (err) {
        console.error('Failed to load PDF:', err);
        setError(`Failed to load PDF: ${err}`);
        setLoading(false);
      }
    };

    loadPdf();

    // Cleanup blob URL on unmount
    return () => {
      if (pdfUrl && pdfUrl.startsWith('blob:')) {
        URL.revokeObjectURL(pdfUrl);
      }
    };
  }, [artifact]);

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="text-center">
          <FileText className="w-16 h-16 mx-auto mb-4 text-shodh-text-faint animate-pulse motion-reduce:animate-none" />
          <p className="text-shodh-text-muted">Loading PDF...</p>
        </div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="text-center max-w-md">
          <AlertCircle className="w-16 h-16 mx-auto mb-4 text-shodh-error" />
          <p className="text-shodh-error mb-2">Failed to load PDF</p>
          <p className="text-body text-shodh-text-muted">{error}</p>
        </div>
      </div>
    );
  }

  // Get citation info
  const pageNumber = artifact.metadata?.pageNumber;
  const snippet = artifact.metadata?.snippet;
  const lineRange = artifact.metadata?.lineRange;

  return (
    <div className="w-full h-full flex flex-col bg-shodh-surface">
      {/* Citation Highlight Banner */}
      {(pageNumber || snippet) && (
        <div className="bg-shodh-warning-soft border-s-4 border-shodh-warning p-3 mb-2 mx-2 mt-2 rounded-e">
          <div className="flex items-start gap-2">
            <div className="flex-shrink-0 mt-0.5">
              <div className="w-5 h-5 rounded-full bg-shodh-accent text-shodh-on-accent flex items-center justify-center text-caption font-bold">
                📍
              </div>
            </div>
            <div className="flex-1 min-w-0">
              <div className="flex items-center gap-2 mb-1">
                <span className="text-caption font-semibold text-shodh-text">
                  CITED SECTION
                </span>
                {pageNumber && (
                  <span className="text-caption bg-shodh-raised-2 text-shodh-text px-2 py-0.5 rounded-full font-medium">
                    Page {pageNumber}
                  </span>
                )}
                {lineRange && (
                  <span className="text-caption bg-shodh-raised-2 text-shodh-text px-2 py-0.5 rounded-full font-medium">
                    Lines {lineRange[0]}-{lineRange[1]}
                  </span>
                )}
              </div>
              {snippet && (
                <p className="text-body text-shodh-text-secondary italic line-clamp-3">
                  "{snippet.length > 200 ? snippet.substring(0, 200) + '...' : snippet}"
                </p>
              )}
            </div>
          </div>
        </div>
      )}

      {/* PDF Viewer */}
      <div className="flex-1 relative">
        <iframe
          src={pdfUrl}
          className="w-full h-full border-0"
          title={artifact.title}
          style={{ minHeight: '600px' }}
        />
      </div>
    </div>
  );
};
