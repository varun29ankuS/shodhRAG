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
import mermaid from 'mermaid';
import type { Artifact } from './EnhancedArtifactPanel';
import { useTheme } from '../contexts/ThemeContext';

interface ArtifactPreviewCardProps {
  artifact: Artifact;
  onClick?: () => void;
}

export function ArtifactPreviewCard({ artifact, onClick }: ArtifactPreviewCardProps) {
  const { colors, theme } = useTheme();
  const mermaidRef = useRef<HTMLDivElement>(null);
  const [mermaidError, setMermaidError] = useState<string | null>(null);

  useEffect(() => {
    mermaid.initialize({
      startOnLoad: false,
      theme: theme === 'dark' ? 'dark' : 'default',
      securityLevel: 'loose',
    });
  }, [theme]);

  const isMermaid = typeof artifact.artifact_type === 'string'
    ? artifact.artifact_type.toLowerCase() === 'mermaid'
    : !!(artifact.artifact_type as any).Mermaid;

  useEffect(() => {
    if (isMermaid && mermaidRef.current) {
      renderMermaid();
    }
  }, [artifact.content, isMermaid]);

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
    const type = typeof artifact.artifact_type === 'string' ? artifact.artifact_type.toLowerCase() : '';
    if (type === 'code' || (artifact.artifact_type as any).Code) return Code;
    if (type === 'table' || (artifact.artifact_type as any).Table) return FileText;
    if (type === 'chart' || (artifact.artifact_type as any).Chart) return FileText;
    if (type === 'markdown' || (artifact.artifact_type as any).Markdown) return FileText;
    if (type === 'mermaid' || (artifact.artifact_type as any).Mermaid) return ImageIcon;
    return FileText;
  };

  const getTypeName = () => {
    const type = typeof artifact.artifact_type === 'string' ? artifact.artifact_type.toLowerCase() : '';
    if (type === 'code' || (artifact.artifact_type as any).Code) {
      const lang = artifact.language || 'code';
      return lang.toUpperCase();
    }
    if (type === 'table' || (artifact.artifact_type as any).Table) return 'TABLE';
    if (type === 'chart' || (artifact.artifact_type as any).Chart) return 'CHART';
    if (type === 'markdown' || (artifact.artifact_type as any).Markdown) return 'MARKDOWN';
    if (type === 'mermaid' || (artifact.artifact_type as any).Mermaid) return 'DIAGRAM';
    if (type === 'svg' || (artifact.artifact_type as any).SVG) return 'SVG';
    if (type === 'html' || (artifact.artifact_type as any).HTML) return 'HTML';
    return 'ARTIFACT';
  };

  const isType = (typeName: string) => {
    const type = typeof artifact.artifact_type === 'string' ? artifact.artifact_type.toLowerCase() : '';
    return type === typeName.toLowerCase() || (artifact.artifact_type as any)[typeName];
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
      className={`my-3 rounded-lg overflow-auto cursor-pointer transition-shadow ${isType('Mermaid') ? 'min-h-[200px]' : 'max-h-[200px] min-h-[100px]'}`}
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
        {isType('Code') && (
          <pre className="text-xs font-mono overflow-auto max-h-[300px] min-h-[150px]" style={{ color: colors.text }}>
            <code>{getPreview()}</code>
          </pre>
        )}

        {isType('Mermaid') && (
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

        {isType('Markdown') && (
          <div className="text-xs line-clamp-8 min-h-[150px] max-h-[300px] overflow-auto" style={{ color: colors.textSecondary }}>
            {getPreview()}
          </div>
        )}

        {isType('Table') && (
          <pre className="text-xs font-mono overflow-auto max-h-[300px] min-h-[100px] whitespace-pre-wrap" style={{ color: colors.text }}>
            {getPreview()}
          </pre>
        )}

        {isType('Chart') && (
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
        {!isType('Code') && !isType('Mermaid') && !isType('Markdown') && !isType('Table') && !isType('Chart') && (
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
