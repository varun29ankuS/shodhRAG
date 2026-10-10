/**
 * Artifact Preview Card
 *
 * Inline preview of artifacts in chat messages
 * - Shows artifact type icon and title
 * - Displays code snippet or diagram preview
 * - Click to expand in full artifact panel
 */

import { motion } from 'framer-motion';
import { Code, FileText, Image as ImageIcon, ExternalLink, Eye } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import type { Artifact } from './EnhancedArtifactPanel';
import { useTheme } from '../contexts/ThemeContext';
import { getArtifactKind, getArtifactCodeLanguage } from '../utils/artifactKind';

interface ArtifactPreviewCardProps {
  artifact: Artifact;
  onClick?: () => void;
}

export function ArtifactPreviewCard({ artifact, onClick }: ArtifactPreviewCardProps) {
  const { colors, theme } = useTheme();
  const mermaidRef = useRef<HTMLDivElement>(null);
  const [mermaidError, setMermaidError] = useState<string | null>(null);

  const kind = getArtifactKind(artifact.artifact_type);
  const isMermaid = kind === 'mermaid';

  useEffect(() => {
    if (isMermaid && mermaidRef.current) {
      renderMermaid();
    }
  }, [artifact.content, isMermaid, theme]);

  const renderMermaid = async () => {
    if (!mermaidRef.current) return;

    try {
      setMermaidError(null);
      mermaidRef.current.innerHTML = '';

      let diagramContent = artifact.content.trim();
      const hasType = /^(graph|flowchart|sequenceDiagram|classDiagram|stateDiagram|erDiagram|journey|gantt|pie|gitGraph)/.test(diagramContent);

      if (!hasType) {
        diagramContent = `flowchart TD\n${diagramContent}`;
      }

      // Loaded on first use: mermaid is several megabytes and most answers have no diagram.
      const { default: mermaid } = await import('mermaid');
      mermaid.initialize({
        startOnLoad: false,
        theme: theme === 'dark' ? 'dark' : 'default',
        securityLevel: 'loose',
      });
      const { svg } = await mermaid.render(
        `mermaid-preview-${artifact.id}-${Date.now()}`,
        diagramContent
      );

      mermaidRef.current.innerHTML = svg;
    } catch (err: any) {
      console.error('Mermaid preview error:', err);
      setMermaidError(err?.message || 'Failed to render');
    }
  };

  const getIcon = () => {
    if (kind === 'code') return Code;
    if (kind === 'mermaid') return ImageIcon;
    return FileText;
  };

  const getTypeName = () => {
    switch (kind) {
      case 'code': return (getArtifactCodeLanguage(artifact) || 'code').toUpperCase();
      case 'table': return 'TABLE';
      case 'chart': return 'CHART';
      case 'markdown': return 'MARKDOWN';
      case 'mermaid': return 'DIAGRAM';
      case 'svg': return 'SVG';
      case 'html': return 'HTML';
      case 'pdf': return 'PDF';
      default: return 'ARTIFACT';
    }
  };

  const getPreview = () => {
    const content = artifact.content.trim();
    const lines = content.split('\n');

    if (lines.length <= 3) {
      return content;
    }

    return lines.slice(0, 3).join('\n') + '\n...';
  };

  const Icon = getIcon();

  return (
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      whileHover={{ scale: 1.02, y: -2 }}
      onClick={onClick}
      className={`my-3 rounded-lg overflow-auto cursor-pointer transition-shadow ${kind === 'mermaid' ? 'min-h-[200px]' : 'max-h-[200px] min-h-[100px]'}`}
      style={{
        backgroundColor: colors.cardBg,
        border: `1px solid ${colors.border}`,
        boxShadow: '0 1px 3px rgba(0,0,0,0.08)',
      }}
    >
      {/* Header */}
      <div
        className="flex items-center justify-between px-4 py-2"
        style={{ backgroundColor: colors.bgSecondary, borderBottom: `1px solid ${colors.border}` }}
      >
        <div className="flex items-center space-x-2">
          <Icon className="w-4 h-4" style={{ color: colors.primary }} />
          <span className="text-xs font-semibold" style={{ color: colors.primary }}>
            {getTypeName()}
          </span>
          <span className="text-xs" style={{ color: colors.textMuted }}>&bull;</span>
          <span className="text-xs font-medium" style={{ color: colors.text }}>
            {artifact.title}
          </span>
        </div>

        <motion.div
          whileHover={{ scale: 1.1 }}
          className="flex items-center space-x-1 text-xs"
          style={{ color: colors.textMuted }}
        >
          <Eye className="w-3 h-3" />
          <span>View</span>
        </motion.div>
      </div>

      {/* Preview Content */}
      <div className="p-3">
        {kind === 'code' && (
          <pre className="text-xs font-mono overflow-auto max-h-[300px] min-h-[150px]" style={{ color: colors.text }}>
            <code>{getPreview()}</code>
          </pre>
        )}

        {kind === 'mermaid' && (
          <div className="min-h-[200px] overflow-auto flex items-center justify-center">
            {mermaidError ? (
              <div
                className="p-4 rounded-lg max-w-md"
                style={{ border: `1px solid ${colors.error}30`, backgroundColor: `${colors.error}08` }}
              >
                <p className="text-sm font-semibold mb-2" style={{ color: colors.error }}>Diagram Error</p>
                <p className="text-xs font-mono whitespace-pre-wrap" style={{ color: `${colors.error}cc` }}>{mermaidError}</p>
                <p className="text-xs mt-2" style={{ color: colors.textMuted }}>Click to view source and fix syntax</p>
              </div>
            ) : (
              <div ref={mermaidRef} className="w-full flex items-center justify-center p-4" />
            )}
          </div>
        )}

        {kind === 'markdown' && (
          <div className="text-xs line-clamp-8 min-h-[150px] max-h-[300px] overflow-auto" style={{ color: colors.textSecondary }}>
            {getPreview()}
          </div>
        )}

        {kind === 'table' && (
          <pre className="text-xs font-mono overflow-auto max-h-[300px] min-h-[100px] whitespace-pre-wrap" style={{ color: colors.text }}>
            {getPreview()}
          </pre>
        )}

        {kind === 'chart' && (
          <div className="flex items-center justify-center min-h-[100px] text-sm" style={{ color: colors.textMuted }}>
            {(() => {
              try {
                const spec = JSON.parse(artifact.content.trim());
                return <span>{spec.type?.toUpperCase() || 'CHART'} — {spec.data?.datasets?.length || 0} dataset(s), {spec.data?.labels?.length || 0} labels &bull; Click to view</span>;
              } catch {
                return <span>Chart — Click to view</span>;
              }
            })()}
          </div>
        )}

        {/* Fallback for unknown types */}
        {kind !== 'code' && kind !== 'mermaid' && kind !== 'markdown' && kind !== 'table' && kind !== 'chart' && (
          <pre className="text-xs font-mono overflow-auto max-h-[300px] min-h-[100px] whitespace-pre-wrap" style={{ color: colors.text }}>
            {getPreview()}
          </pre>
        )}
      </div>

      {/* Footer */}
      <div
        className="px-4 py-2 flex items-center justify-between"
        style={{ backgroundColor: colors.bgSecondary, borderTop: `1px solid ${colors.border}` }}
      >
        <span className="text-xs" style={{ color: colors.textMuted }}>
          {artifact.content.split('\n').length} lines
        </span>
        <motion.button
          whileHover={{ scale: 1.05 }}
          whileTap={{ scale: 0.95 }}
          className="flex items-center space-x-1 text-xs"
          style={{ color: colors.primary }}
        >
          <ExternalLink className="w-3 h-3" />
          <span>Expand</span>
        </motion.button>
      </div>
    </motion.div>
  );
}

export default ArtifactPreviewCard;
