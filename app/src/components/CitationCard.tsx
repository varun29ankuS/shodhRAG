import React from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ExternalLink, FileText } from 'lucide-react';
import { useTheme } from '../contexts/ThemeContext';

interface Citation {
  title: string;
  authors: string[];
  source: string;
  year: string;
  url?: string;
  doi?: string;
  pageNumbers?: string;
}

interface CitationCardProps {
  citation: Citation;
  sourceFile: string;
  lineRange?: [number, number];
  pageNumber?: number;
  snippet: string;
  score: number;
  index: number;
}

export const CitationCard: React.FC<CitationCardProps> = ({
  citation,
  sourceFile,
  lineRange,
  pageNumber,
  snippet,
  score,
  index,
}) => {
  const { colors } = useTheme();

  const handleClick = async () => {
    try {
      const lineNumber = lineRange ? lineRange[0] : undefined;
      await invoke('open_file_at_location', {
        filePath: sourceFile,
        lineNumber,
        pageNumber,
      });
      console.log(`Opened ${sourceFile} at line ${lineNumber || 'start'}`);
    } catch (error) {
      console.error('Failed to open file:', error);
      alert(`Could not open file: ${error}`);
    }
  };

  return (
    <div
      onClick={handleClick}
      className="border-2 rounded-lg p-3 mb-2 cursor-pointer transition-all hover:shadow-lg hover:scale-[1.01]"
      style={{
        backgroundColor: colors.bgSecondary,
        borderColor: colors.border,
      }}
    >
      {/* Header with citation number and score */}
      <div className="flex justify-between items-start mb-2">
        <div className="flex items-center gap-2">
          <div
            className="w-6 h-6 rounded-full flex items-center justify-center text-xs font-bold"
            style={{
              backgroundColor: colors.primary,
              color: '#ffffff',
            }}
          >
            {index + 1}
          </div>
          <FileText className="w-4 h-4" style={{ color: colors.primary }} />
          <span className="text-sm font-bold" style={{ color: colors.text }}>
            {citation.title || 'Document'}
          </span>
        </div>
        <div className="flex items-center gap-2">
          <span
            className="text-xs px-2 py-0.5 rounded"
            style={{
              backgroundColor: `${colors.primary}20`,
              color: colors.primary,
            }}
          >
            {(score * 100).toFixed(1)}%
          </span>
          <ExternalLink className="w-3 h-3" style={{ color: colors.textMuted }} />
        </div>
      </div>

      {/* Snippet */}
      <p
        className="text-xs mb-2 line-clamp-2"
        style={{ color: colors.textSecondary }}
      >
        {snippet}
      </p>

      {/* Metadata */}
      <div className="flex flex-wrap gap-2 text-xs" style={{ color: colors.textMuted }}>
        {citation.authors && citation.authors.length > 0 && (
          <span>{citation.authors.slice(0, 2).join(', ')}</span>
        )}
        {citation.year && <span>• {citation.year}</span>}
        {pageNumber && <span>• Page {pageNumber}</span>}
        {lineRange && <span>• Lines {lineRange[0]}-{lineRange[1]}</span>}
      </div>

      {/* Hover hint */}
      <div
        className="text-xs mt-2 pt-2 border-t"
        style={{
          borderColor: colors.border,
          color: colors.textMuted,
        }}
      >
        Click to open in editor
      </div>
    </div>
  );
};
