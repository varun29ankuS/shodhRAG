import React from 'react';
import { invoke } from '@tauri-apps/api/core';
import { FileText, ExternalLink, HelpCircle, CheckCircle, FileSearch, BookOpen, X, Layers } from 'lucide-react';
import { useTheme } from '../contexts/ThemeContext';

export interface Citation {
  id: string;
  sourceFile: string;
  pageNumber?: number;
  lineRange?: [number, number];
  snippet: string;
  surroundingContext: string;
  citationTitle: string;
}

interface CitationBadgeProps {
  citation: Citation;
  index: number;
  onFollowUpQuery?: (query: string) => void;
  onOpenUrl?: (url: string) => void;
  onViewInArtifact?: (citation: Citation) => void;
}

export const CitationBadge: React.FC<CitationBadgeProps> = ({ citation, index, onFollowUpQuery, onOpenUrl, onViewInArtifact }) => {
  const { colors } = useTheme();
  const [isExpanded, setIsExpanded] = React.useState(false);
  const [isJumping, setIsJumping] = React.useState(false);
  const [contextMenu, setContextMenu] = React.useState<{ x: number; y: number } | null>(null);
  const popoverRef = React.useRef<HTMLDivElement>(null);

  const isUrl = citation.sourceFile.startsWith('http://') || citation.sourceFile.startsWith('https://');

  // Close popover on outside click
  React.useEffect(() => {
    if (!isExpanded) return;
    const handler = (e: MouseEvent) => {
      if (popoverRef.current && !popoverRef.current.contains(e.target as Node)) {
        setIsExpanded(false);
      }
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [isExpanded]);

  const handleJumpToSource = async () => {
    setIsJumping(true);
    try {
      if (isUrl && onOpenUrl) {
        onOpenUrl(citation.sourceFile);
        setIsExpanded(false);
      } else {
        await invoke('jump_to_source', {
          filePath: citation.sourceFile,
          lineNumber: citation.lineRange ? citation.lineRange[0] : undefined,
          pageNumber: citation.pageNumber,
          searchText: citation.snippet,
        });
      }
    } catch (error) {
      console.error('Failed to jump to source:', error);
    } finally {
      setIsJumping(false);
    }
  };

  const handleRightClick = (e: React.MouseEvent) => {
    e.preventDefault();
    setContextMenu({ x: e.clientX, y: e.clientY });
  };

  const handleFollowUp = (action: string) => {
    const fileName = citation.citationTitle || getFileName();
    const location = citation.pageNumber ? `page ${citation.pageNumber}` :
                     citation.lineRange ? `lines ${citation.lineRange[0]}-${citation.lineRange[1]}` : 'this section';

    const queries: Record<string, string> = {
      'explain': `Explain what this means: "${citation.snippet}"`,
      'verify': `Is this information accurate? Verify: "${citation.snippet}"`,
      'show-full-section': `Show me the full section from ${fileName} (${location})`,
      'related-info': `What else does ${fileName} say about this topic?`,
    };

    if (onFollowUpQuery && queries[action]) {
      onFollowUpQuery(queries[action]);
    }
    setContextMenu(null);
  };

  React.useEffect(() => {
    const handleClickOutside = () => setContextMenu(null);
    if (contextMenu) {
      document.addEventListener('click', handleClickOutside);
      return () => document.removeEventListener('click', handleClickOutside);
    }
  }, [contextMenu]);

  const getLocationText = () => {
    if (citation.lineRange) return `Lines ${citation.lineRange[0]}-${citation.lineRange[1]}`;
    if (citation.pageNumber) return `Page ${citation.pageNumber}`;
    return 'Source Document';
  };

  const getFileName = () => {
    const parts = citation.sourceFile.split(/[/\\]/);
    return parts[parts.length - 1];
  };

  return (
    <span style={{ display: 'inline-block', position: 'relative', verticalAlign: 'super', fontSize: '0.75em', lineHeight: 0 }}>
      {/* Citation number badge */}
      <button
        onClick={() => setIsExpanded(!isExpanded)}
        onContextMenu={handleRightClick}
        title={`${citation.citationTitle} (Click for details)`}
        style={{
          display: 'inline-flex',
          alignItems: 'center',
          justifyContent: 'center',
          width: '16px',
          height: '16px',
          borderRadius: '4px',
          backgroundColor: colors.primary,
          color: '#ffffff',
          fontSize: '9px',
          fontWeight: 700,
          border: 'none',
          cursor: 'pointer',
          padding: 0,
          margin: '0 1px',
          transition: 'all 0.15s ease',
          verticalAlign: 'baseline',
          lineHeight: 1,
        }}
        onMouseEnter={(e) => {
          e.currentTarget.style.transform = 'scale(1.15)';
        }}
        onMouseLeave={(e) => {
          e.currentTarget.style.transform = 'scale(1)';
        }}
      >
        {index}
      </button>

      {/* Popover panel */}
      {isExpanded && (
        <div
          ref={popoverRef}
          style={{
            position: 'absolute',
            zIndex: 50,
            marginTop: '8px',
            width: '380px',
            maxWidth: '90vw',
            left: 0,
            backgroundColor: colors.cardBg,
            border: `1px solid ${colors.border}`,
            borderRadius: '12px',
            boxShadow: '0 8px 30px rgba(0,0,0,0.12), 0 2px 8px rgba(0,0,0,0.06)',
            overflow: 'hidden',
          }}
        >
          {/* Header */}
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'space-between',
              padding: '10px 12px',
              borderBottom: `1px solid ${colors.border}`,
              backgroundColor: colors.bgSecondary,
            }}
          >
            <div style={{ display: 'flex', alignItems: 'center', gap: '8px', flex: 1, minWidth: 0 }}>
              <FileText size={14} style={{ color: colors.primary, flexShrink: 0 }} />
              <span style={{ fontSize: '12px', fontWeight: 600, color: colors.text, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                {citation.citationTitle}
              </span>
            </div>
            <button
              onClick={(e) => { e.stopPropagation(); setIsExpanded(false); }}
              style={{
                background: 'none',
                border: 'none',
                cursor: 'pointer',
                padding: '2px',
                color: colors.textMuted,
                display: 'flex',
                flexShrink: 0,
                borderRadius: '4px',
              }}
              onMouseEnter={e => (e.currentTarget.style.backgroundColor = colors.bgHover)}
              onMouseLeave={e => (e.currentTarget.style.backgroundColor = 'transparent')}
            >
              <X size={14} />
            </button>
          </div>

          {/* Content */}
          <div style={{ padding: '10px 12px' }}>
            {/* Action buttons row */}
            <div style={{ display: 'flex', gap: '6px', marginBottom: '10px' }}>
              {/* Open source */}
              <button
                onClick={handleJumpToSource}
                disabled={isJumping}
                style={{
                  flex: 1,
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'center',
                  gap: '6px',
                  padding: '7px 10px',
                  borderRadius: '8px',
                  border: `1px solid ${colors.border}`,
                  backgroundColor: 'transparent',
                  color: colors.text,
                  fontSize: '11px',
                  fontWeight: 500,
                  cursor: 'pointer',
                  transition: 'all 0.15s ease',
                  opacity: isJumping ? 0.5 : 1,
                }}
                onMouseEnter={e => {
                  e.currentTarget.style.backgroundColor = colors.bgHover;
                  e.currentTarget.style.borderColor = colors.primary;
                }}
                onMouseLeave={e => {
                  e.currentTarget.style.backgroundColor = 'transparent';
                  e.currentTarget.style.borderColor = colors.border;
                }}
              >
                <ExternalLink size={12} />
                <span>{isJumping ? 'Opening...' : isUrl ? 'Open URL' : 'Open File'}</span>
              </button>

              {/* View in Artifacts */}
              {onViewInArtifact && !isUrl && (
                <button
                  onClick={() => {
                    onViewInArtifact(citation);
                    setIsExpanded(false);
                  }}
                  style={{
                    flex: 1,
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    gap: '6px',
                    padding: '7px 10px',
                    borderRadius: '8px',
                    border: `1px solid ${colors.border}`,
                    backgroundColor: 'transparent',
                    color: colors.text,
                    fontSize: '11px',
                    fontWeight: 500,
                    cursor: 'pointer',
                    transition: 'all 0.15s ease',
                  }}
                  onMouseEnter={e => {
                    e.currentTarget.style.backgroundColor = '#ef444410';
                    e.currentTarget.style.borderColor = '#ef444440';
                  }}
                  onMouseLeave={e => {
                    e.currentTarget.style.backgroundColor = 'transparent';
                    e.currentTarget.style.borderColor = colors.border;
                  }}
                >
                  <Layers size={12} style={{ color: '#ef4444' }} />
                  <span>View in Artifacts</span>
                </button>
              )}
            </div>

            {/* Location chip */}
            <div
              style={{
                display: 'inline-flex',
                alignItems: 'center',
                gap: '4px',
                fontSize: '10px',
                color: colors.textMuted,
                backgroundColor: colors.bgTertiary,
                padding: '2px 8px',
                borderRadius: '4px',
                marginBottom: '8px',
              }}
            >
              {getFileName()} &middot; {getLocationText()}
            </div>

            {/* Snippet */}
            {citation.snippet && (
              <div>
                <div style={{ fontSize: '9px', fontWeight: 600, color: colors.textMuted, textTransform: 'uppercase', letterSpacing: '0.5px', marginBottom: '4px' }}>
                  Relevant excerpt
                </div>
                <div
                  style={{
                    fontSize: '11px',
                    lineHeight: 1.6,
                    padding: '8px 10px',
                    borderRadius: '8px',
                    backgroundColor: colors.bgTertiary,
                    borderLeft: `2px solid ${colors.primary}`,
                    color: colors.textSecondary,
                    fontStyle: 'italic',
                  }}
                >
                  &ldquo;{citation.snippet}&rdquo;
                </div>
              </div>
            )}

            {/* Surrounding context (expandable) */}
            {citation.surroundingContext && citation.surroundingContext !== citation.snippet && (
              <details style={{ marginTop: '8px' }}>
                <summary
                  style={{
                    fontSize: '10px',
                    fontWeight: 500,
                    color: colors.primary,
                    cursor: 'pointer',
                    userSelect: 'none',
                  }}
                >
                  Show more context
                </summary>
                <div
                  style={{
                    marginTop: '6px',
                    padding: '8px 10px',
                    borderRadius: '8px',
                    backgroundColor: colors.bgTertiary,
                    border: `1px solid ${colors.border}`,
                    fontSize: '11px',
                    lineHeight: 1.6,
                    color: colors.textSecondary,
                    maxHeight: '200px',
                    overflowY: 'auto',
                  }}
                >
                  {citation.surroundingContext}
                </div>
              </details>
            )}
          </div>
        </div>
      )}

      {/* Context Menu (right-click) */}
      {contextMenu && (
        <div
          style={{
            position: 'fixed',
            zIndex: 9999,
            top: contextMenu.y,
            left: contextMenu.x,
            minWidth: '200px',
            backgroundColor: colors.cardBg,
            border: `1px solid ${colors.border}`,
            borderRadius: '10px',
            boxShadow: '0 8px 30px rgba(0,0,0,0.15)',
            padding: '4px',
            backdropFilter: 'blur(8px)',
          }}
          onClick={(e) => e.stopPropagation()}
        >
          {[
            { key: 'explain', icon: HelpCircle, label: 'Explain this' },
            { key: 'verify', icon: CheckCircle, label: 'Verify accuracy' },
            { key: 'show-full-section', icon: FileSearch, label: 'Show full section' },
            { key: 'related-info', icon: BookOpen, label: 'Related information' },
          ].map(item => (
            <button
              key={item.key}
              onClick={() => handleFollowUp(item.key)}
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: '8px',
                width: '100%',
                padding: '7px 10px',
                border: 'none',
                backgroundColor: 'transparent',
                color: colors.text,
                fontSize: '12px',
                fontWeight: 500,
                cursor: 'pointer',
                borderRadius: '6px',
                textAlign: 'left',
                transition: 'background-color 0.1s ease',
              }}
              onMouseEnter={e => (e.currentTarget.style.backgroundColor = colors.bgHover)}
              onMouseLeave={e => (e.currentTarget.style.backgroundColor = 'transparent')}
            >
              <item.icon size={13} style={{ color: colors.textMuted }} />
              {item.label}
            </button>
          ))}

          <div style={{ height: '1px', backgroundColor: colors.border, margin: '4px 8px' }} />

          <button
            onClick={() => { handleJumpToSource(); setContextMenu(null); }}
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: '8px',
              width: '100%',
              padding: '7px 10px',
              border: 'none',
              backgroundColor: 'transparent',
              color: colors.primary,
              fontSize: '12px',
              fontWeight: 600,
              cursor: 'pointer',
              borderRadius: '6px',
              textAlign: 'left',
              transition: 'background-color 0.1s ease',
            }}
            onMouseEnter={e => (e.currentTarget.style.backgroundColor = `${colors.primary}0a`)}
            onMouseLeave={e => (e.currentTarget.style.backgroundColor = 'transparent')}
          >
            <ExternalLink size={13} />
            {isUrl ? 'Open in Browser' : 'Open in Editor'}
          </button>
        </div>
      )}
    </span>
  );
};
