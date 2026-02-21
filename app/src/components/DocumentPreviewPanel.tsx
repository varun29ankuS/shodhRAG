import React, { useState, useEffect } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { readTextFile } from '@tauri-apps/plugin-fs';
import {
  X,
  ExternalLink,
  FileText,
  Code,
  FileSpreadsheet,
  Presentation,
  BookOpen,
  Maximize2,
  Minimize2,
} from 'lucide-react';
import { useTheme } from '../contexts/ThemeContext';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import { oneDark, oneLight } from 'react-syntax-highlighter/dist/esm/styles/prism';

interface PreviewFile {
  path: string;
  name: string;
  page?: number;
}

interface DocumentPreviewPanelProps {
  file: PreviewFile | null;
  onClose: () => void;
}

function getFileExtension(path: string): string {
  const parts = path.split('.');
  return parts.length > 1 ? parts[parts.length - 1].toLowerCase() : '';
}

function getFileTypeBadge(ext: string): { label: string; color: string; Icon: React.ElementType } {
  switch (ext) {
    case 'pdf': return { label: 'PDF', color: '#ef4444', Icon: FileText };
    case 'md': case 'markdown': return { label: 'MD', color: '#8b5cf6', Icon: BookOpen };
    case 'txt': return { label: 'TXT', color: '#6b7280', Icon: FileText };
    case 'csv': return { label: 'CSV', color: '#059669', Icon: FileSpreadsheet };
    case 'json': return { label: 'JSON', color: '#f59e0b', Icon: Code };
    case 'html': case 'htm': return { label: 'HTML', color: '#e34c26', Icon: Code };
    case 'js': case 'jsx': case 'ts': case 'tsx': return { label: ext.toUpperCase(), color: '#3178c6', Icon: Code };
    case 'py': return { label: 'PY', color: '#3776ab', Icon: Code };
    case 'rs': return { label: 'RS', color: '#f74c00', Icon: Code };
    case 'docx': case 'doc': return { label: 'DOCX', color: '#2b579a', Icon: FileText };
    case 'xlsx': case 'xls': return { label: 'XLSX', color: '#217346', Icon: FileSpreadsheet };
    case 'pptx': case 'ppt': return { label: 'PPTX', color: '#d24726', Icon: Presentation };
    default: return { label: ext.toUpperCase() || 'FILE', color: '#9ca3af', Icon: FileText };
  }
}

const codeExtensions = new Set([
  'js', 'jsx', 'ts', 'tsx', 'py', 'rs', 'go', 'java', 'c', 'cpp', 'h', 'hpp',
  'cs', 'rb', 'php', 'swift', 'kt', 'sh', 'bash', 'zsh', 'yaml', 'yml',
  'toml', 'xml', 'sql', 'css', 'scss', 'sass', 'html', 'htm', 'json', 'vue', 'svelte',
]);

const textExtensions = new Set(['md', 'markdown', 'txt', 'csv', 'log', 'ini', 'cfg', 'conf', 'env']);

export default function DocumentPreviewPanel({ file, onClose }: DocumentPreviewPanelProps) {
  const { theme, colors } = useTheme();
  const [content, setContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState(false);

  useEffect(() => {
    if (!file) {
      setContent(null);
      setError(null);
      return;
    }

    const ext = getFileExtension(file.path);

    // PDF: no text loading needed (use iframe)
    if (ext === 'pdf') {
      setContent(null);
      setError(null);
      return;
    }

    // Binary formats: can't preview inline
    if (['docx', 'doc', 'xlsx', 'xls', 'pptx', 'ppt'].includes(ext)) {
      setContent(null);
      setError(null);
      return;
    }

    // Text/code files: load content
    if (codeExtensions.has(ext) || textExtensions.has(ext)) {
      setLoading(true);
      setError(null);
      readTextFile(file.path)
        .then(text => {
          setContent(text);
          setLoading(false);
        })
        .catch(err => {
          setError(`Failed to read file: ${err}`);
          setLoading(false);
        });
      return;
    }

    // Unknown: try reading as text
    setLoading(true);
    readTextFile(file.path)
      .then(text => {
        setContent(text);
        setLoading(false);
      })
      .catch(() => {
        setError('Cannot preview this file type');
        setLoading(false);
      });
  }, [file]);

  useEffect(() => {
    const handleEscape = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && file) {
        onClose();
      }
    };
    window.addEventListener('keydown', handleEscape);
    return () => window.removeEventListener('keydown', handleEscape);
  }, [file, onClose]);

  if (!file) return null;

  const ext = getFileExtension(file.path);
  const { label, color, Icon } = getFileTypeBadge(ext);
  const isMarkdown = ext === 'md' || ext === 'markdown';
  const isCode = codeExtensions.has(ext) && !isMarkdown;
  const isPdf = ext === 'pdf';
  const isBinary = ['docx', 'doc', 'xlsx', 'xls', 'pptx', 'ppt'].includes(ext);
  const panelWidth = expanded ? 'w-[65%]' : 'w-[45%]';

  return (
    <>
      {/* Backdrop */}
      <motion.div
        initial={{ opacity: 0 }}
        animate={{ opacity: 0.3 }}
        exit={{ opacity: 0 }}
        transition={{ duration: 0.2 }}
        className="fixed inset-0 bg-black z-40"
        onClick={onClose}
      />

      {/* Panel */}
      <motion.div
        initial={{ x: '100%' }}
        animate={{ x: 0 }}
        exit={{ x: '100%' }}
        transition={{ type: 'spring', damping: 25, stiffness: 200 }}
        className={`fixed right-0 top-0 bottom-0 ${panelWidth} z-50 flex flex-col`}
        style={{
          backgroundColor: colors.bg,
          boxShadow: '-4px 0 24px rgba(0,0,0,0.2)',
        }}
      >
        {/* Header */}
        <div
          className="flex items-center gap-3 px-4 py-3 border-b shrink-0"
          style={{ borderColor: colors.border }}
        >
          <Icon className="w-4 h-4 shrink-0" style={{ color }} />
          <span
            className="text-[10px] font-bold px-1.5 py-0.5 rounded shrink-0"
            style={{ backgroundColor: `${color}20`, color }}
          >
            {label}
          </span>
          <span className="text-sm font-medium truncate flex-1" style={{ color: colors.text }}>
            {file.name}
          </span>
          <div className="flex items-center gap-1 shrink-0">
            <button
              onClick={() => setExpanded(!expanded)}
              className="w-7 h-7 rounded-md flex items-center justify-center transition-colors"
              style={{ color: colors.textTertiary }}
              title={expanded ? 'Shrink panel' : 'Expand panel'}
            >
              {expanded ? <Minimize2 className="w-3.5 h-3.5" /> : <Maximize2 className="w-3.5 h-3.5" />}
            </button>
            <button
              onClick={onClose}
              className="w-7 h-7 rounded-md flex items-center justify-center transition-colors"
              style={{ color: colors.textTertiary }}
              title="Close preview (Esc)"
            >
              <X className="w-4 h-4" />
            </button>
          </div>
        </div>

        {/* Content */}
        <div className="flex-1 overflow-auto">
          {loading && (
            <div className="flex items-center justify-center h-full">
              <div className="text-center">
                <div
                  className="w-6 h-6 border-2 border-t-transparent rounded-full animate-spin mx-auto mb-2"
                  style={{ borderColor: colors.primary }}
                />
                <p className="text-xs" style={{ color: colors.textMuted }}>Loading file...</p>
              </div>
            </div>
          )}

          {error && (
            <div className="flex items-center justify-center h-full">
              <p className="text-sm" style={{ color: colors.error }}>{error}</p>
            </div>
          )}

          {isPdf && (
            <iframe
              src={`file://${file.path}${file.page ? `#page=${file.page}` : ''}`}
              className="w-full h-full border-none"
              title={file.name}
            />
          )}

          {isBinary && !loading && !error && (
            <div className="flex items-center justify-center h-full">
              <div className="text-center space-y-3">
                <Icon className="w-12 h-12 mx-auto" style={{ color: colors.textMuted }} />
                <p className="text-sm" style={{ color: colors.textSecondary }}>
                  Preview not available for {label} files
                </p>
                <p className="text-xs" style={{ color: colors.textMuted }}>
                  {file.path}
                </p>
              </div>
            </div>
          )}

          {isMarkdown && content !== null && (
            <div className="p-6 prose prose-sm dark:prose-invert max-w-none" style={{ color: colors.text }}>
              <ReactMarkdown remarkPlugins={[remarkGfm]}>
                {content}
              </ReactMarkdown>
            </div>
          )}

          {isCode && content !== null && (
            <SyntaxHighlighter
              style={theme === 'dark' ? oneDark : oneLight}
              language={ext === 'jsx' ? 'jsx' : ext === 'tsx' ? 'tsx' : ext === 'ts' ? 'typescript' : ext === 'js' ? 'javascript' : ext === 'py' ? 'python' : ext === 'rs' ? 'rust' : ext === 'go' ? 'go' : ext === 'java' ? 'java' : ext === 'cpp' || ext === 'cc' || ext === 'cxx' ? 'cpp' : ext === 'c' || ext === 'h' ? 'c' : ext === 'css' ? 'css' : ext === 'scss' ? 'scss' : ext === 'html' || ext === 'htm' ? 'html' : ext === 'json' ? 'json' : ext === 'yaml' || ext === 'yml' ? 'yaml' : ext === 'toml' ? 'toml' : ext === 'sql' ? 'sql' : ext === 'xml' ? 'xml' : ext === 'sh' || ext === 'bash' || ext === 'zsh' ? 'bash' : ext === 'swift' ? 'swift' : ext === 'kt' ? 'kotlin' : ext === 'rb' ? 'ruby' : ext === 'php' ? 'php' : ext}
              showLineNumbers
              customStyle={{
                margin: 0,
                padding: '1rem',
                fontSize: '0.75rem',
                lineHeight: '1.6',
                background: theme === 'dark' ? colors.bgTertiary : '#f8f9fa',
              }}
            >
              {content}
            </SyntaxHighlighter>
          )}

          {!isMarkdown && !isCode && !isPdf && !isBinary && content !== null && (
            <pre
              className="p-6 text-sm leading-relaxed whitespace-pre-wrap font-mono"
              style={{ color: colors.textSecondary }}
            >
              {content}
            </pre>
          )}
        </div>

        {/* Footer */}
        <div
          className="flex items-center justify-between px-4 py-2 border-t text-[10px] shrink-0"
          style={{ borderColor: colors.border, color: colors.textMuted }}
        >
          <span className="truncate" title={file.path}>{file.path}</span>
          {content !== null && (
            <span className="shrink-0 ml-2">{content.length.toLocaleString()} chars</span>
          )}
        </div>
      </motion.div>
    </>
  );
}
