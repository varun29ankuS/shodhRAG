import React, { useState } from 'react';
import { FileText, ExternalLink, ChevronDown, ChevronUp, Layers } from 'lucide-react';
import { useTheme } from '../contexts/ThemeContext';
import { invoke } from '@tauri-apps/api/core';

interface CitationInfo {
  number: number;
  title: string;
  authors?: string[];
  source: string;
  year?: string;
  url?: string;
  sourceFile: string;
  pageNumber?: number;
  lineRange?: [number, number];
  snippet: string;
}

interface CitationFootnotesProps {
  citations: CitationInfo[];
  onOpenSource?: (sourceFile: string, lineNumber?: number, pageNumber?: number) => void;
  onViewInArtifact?: (citation: any) => void;
}

export const CitationFootnotes: React.FC<CitationFootnotesProps> = ({ citations, onViewInArtifact }) => {
  const { colors } = useTheme();
  const [expandedId, setExpandedId] = useState<number | null>(null);

  if (citations.length === 0) return null;

  const handleOpenSource = async (citation: CitationInfo, e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      const isUrl = citation.sourceFile.startsWith('http://') || citation.sourceFile.startsWith('https://');
      if (isUrl) {
        window.open(citation.url || citation.sourceFile, '_blank');
      } else {
        await invoke('jump_to_source', {
          filePath: citation.sourceFile,
          lineNumber: citation.lineRange ? citation.lineRange[0] : undefined,
          pageNumber: citation.pageNumber,
          searchText: citation.snippet,
        });
      }
    } catch (error) {
      console.error('Failed to open source:', error);
    }
  };

  const handleViewInArtifact = (citation: CitationInfo, e: React.MouseEvent) => {
    e.stopPropagation();
    if (!onViewInArtifact) return;
    onViewInArtifact({
      sourceFile: citation.sourceFile,
      citationTitle: citation.title,
      pageNumber: citation.pageNumber,
      lineRange: citation.lineRange,
      snippet: citation.snippet,
    });
  };

  const getFileName = (filePath: string) => {
    const parts = filePath.split(/[/\\]/);
    return parts[parts.length - 1];
  };

  const getLocationText = (citation: CitationInfo) => {
    if (citation.lineRange) return `L${citation.lineRange[0]}-${citation.lineRange[1]}`;
    if (citation.pageNumber) return `p.${citation.pageNumber}`;
    return '';
  };

  return (
    <div style={{ marginTop: '16px', paddingTop: '12px', borderTop: `1px solid ${colors.border}` }}>
      {/* Header */}
      <div
        style={{
          fontSize: '11px',
          fontWeight: 600,
          color: colors.textMuted,
          marginBottom: '8px',
          display: 'flex',
          alignItems: 'center',
          gap: '5px',
          textTransform: 'uppercase',
          letterSpacing: '0.5px',
        }}
      >
        <FileText size={12} />
        Sources ({citations.length})
      </div>

      {/* Compact citation chips */}
      <div style={{ display: 'flex', flexDirection: 'column', gap: '4px' }}>
        {citations.map((citation) => {
          const isExpanded = expandedId === citation.number;
          const fileName = getFileName(citation.sourceFile);
          const location = getLocationText(citation);

          return (
            <div key={citation.number}>
              {/* Compact row */}
              <div
                onClick={() => setExpandedId(isExpanded ? null : citation.number)}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: '8px',
                  padding: '5px 8px',
                  borderRadius: '8px',
                  cursor: 'pointer',
                  backgroundColor: isExpanded ? `${colors.primary}08` : 'transparent',
                  border: `1px solid ${isExpanded ? `${colors.primary}20` : 'transparent'}`,
                  transition: 'all 0.15s ease',
                }}
                onMouseEnter={e => {
                  if (!isExpanded) e.currentTarget.style.backgroundColor = colors.bgHover;
                }}
                onMouseLeave={e => {
                  if (!isExpanded) e.currentTarget.style.backgroundColor = 'transparent';
                }}
              >
                {/* Number badge */}
                <span
                  style={{
                    display: 'inline-flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    minWidth: '16px',
                    height: '16px',
                    borderRadius: '4px',
                    backgroundColor: colors.primary,
                    color: '#ffffff',
                    fontSize: '9px',
                    fontWeight: 700,
                    flexShrink: 0,
                  }}
                >
                  {citation.number}
                </span>

                {/* Title — truncated */}
                <span
                  style={{
                    fontSize: '11px',
                    fontWeight: 500,
                    color: colors.text,
                    flex: 1,
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                    whiteSpace: 'nowrap',
                  }}
                >
                  {citation.title}
                </span>

                {/* File name chip */}
                <span
                  style={{
                    fontSize: '9px',
                    color: colors.textMuted,
                    backgroundColor: colors.bgTertiary,
                    padding: '1px 6px',
                    borderRadius: '4px',
                    flexShrink: 0,
                    maxWidth: '120px',
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                    whiteSpace: 'nowrap',
                  }}
                >
                  {fileName}{location ? ` · ${location}` : ''}
                </span>

                {/* View in Artifacts button */}
                {onViewInArtifact && (
                  <button
                    onClick={(e) => handleViewInArtifact(citation, e)}
                    style={{
                      background: 'none',
                      border: 'none',
                      padding: '2px',
                      cursor: 'pointer',
                      color: '#ef4444',
                      flexShrink: 0,
                      display: 'flex',
                      alignItems: 'center',
                    }}
                    title="View in Artifacts"
                  >
                    <Layers size={11} />
                  </button>
                )}

                {/* Open externally button */}
                <button
                  onClick={(e) => handleOpenSource(citation, e)}
                  style={{
                    background: 'none',
                    border: 'none',
                    padding: '2px',
                    cursor: 'pointer',
                    color: colors.primary,
                    flexShrink: 0,
                    display: 'flex',
                    alignItems: 'center',
                  }}
                  title="Open source file"
                >
                  <ExternalLink size={11} />
                </button>

                {/* Expand toggle */}
                {citation.snippet && (
                  <span style={{ color: colors.textMuted, flexShrink: 0, display: 'flex' }}>
                    {isExpanded ? <ChevronUp size={12} /> : <ChevronDown size={12} />}
                  </span>
                )}
              </div>

              {/* Expanded detail */}
              {isExpanded && (
                <div
                  style={{
                    marginLeft: '32px',
                    marginTop: '4px',
                    marginBottom: '4px',
                  }}
                >
                  {citation.authors && citation.authors.length > 0 && (
                    <div style={{ fontSize: '10px', color: colors.textMuted, marginBottom: '4px' }}>
                      {citation.authors.slice(0, 3).join(', ')}
                      {citation.authors.length > 3 ? ' et al.' : ''}
                      {citation.year ? ` · ${citation.year}` : ''}
                    </div>
                  )}
                  {citation.snippet && (
                    <div
                      style={{
                        padding: '6px 8px',
                        backgroundColor: colors.bgTertiary,
                        border: `1px solid ${colors.border}`,
                        borderRadius: '6px',
                        fontSize: '10px',
                        color: colors.textMuted,
                        fontStyle: 'italic',
                        lineHeight: 1.5,
                        display: '-webkit-box',
                        WebkitLineClamp: 3,
                        WebkitBoxOrient: 'vertical',
                        overflow: 'hidden',
                      }}
                    >
                      &ldquo;{citation.snippet}&rdquo;
                    </div>
                  )}

                  {/* View in Artifacts — expanded action */}
                  {onViewInArtifact && (
                    <button
                      onClick={(e) => handleViewInArtifact(citation, e)}
                      style={{
                        marginTop: '6px',
                        display: 'inline-flex',
                        alignItems: 'center',
                        gap: '4px',
                        background: 'none',
                        border: `1px solid ${colors.border}`,
                        borderRadius: '6px',
                        padding: '3px 8px',
                        cursor: 'pointer',
                        fontSize: '10px',
                        fontWeight: 500,
                        color: colors.text,
                        transition: 'all 0.15s ease',
                      }}
                      onMouseEnter={e => {
                        e.currentTarget.style.backgroundColor = `#ef444412`;
                        e.currentTarget.style.borderColor = '#ef444440';
                        e.currentTarget.style.color = '#ef4444';
                      }}
                      onMouseLeave={e => {
                        e.currentTarget.style.backgroundColor = 'transparent';
                        e.currentTarget.style.borderColor = colors.border;
                        e.currentTarget.style.color = colors.text;
                      }}
                    >
                      <Layers size={10} style={{ color: '#ef4444' }} />
                      View in Artifacts
                    </button>
                  )}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
};
