/**
 * Enhanced Artifact Panel — Slide-Over Drawer with Grouped Sidebar
 *
 * Features:
 * - Vertical sidebar grouped by artifact type (Charts, Tables, Diagrams, Code, Documents)
 * - Content viewer for selected artifact
 * - Copy, download, edit actions
 * - Accepts selectedArtifactId to auto-select on open
 */

import React, { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { save } from '@tauri-apps/plugin-dialog';
import { writeFile } from '@tauri-apps/plugin-fs';
import { motion, AnimatePresence } from 'framer-motion';
import {
  Code, FileText, Image as ImageIcon, Download, Copy, Edit,
  Check, X, Save as SaveIcon, Maximize2, Minimize2, FileCode,
  Table, BarChart, ChevronDown, ChevronRight as ChevronRightIcon
} from 'lucide-react';
import { CodeArtifact } from './CodeArtifact';
import { MarkdownArtifact } from './MarkdownArtifact';
import { MermaidArtifact } from './MermaidArtifact';
import { PDFArtifact } from './PDFArtifact';
import { TableArtifact } from './TableArtifact';
import { ChartArtifact } from './ChartArtifact';
import { useTheme } from '../contexts/ThemeContext';

export interface Artifact {
  id: string;
  artifact_type: {
    Code?: { language: string } | null;
    Markdown?: null;
    Mermaid?: { diagram_type: string } | null;
    Chart?: null;
    Table?: null;
    SVG?: null;
    HTML?: null;
    PDF?: null;
  };
  title: string;
  content: string;
  language?: string;
  editable: boolean;
  version: number;
  created_at: string;
  metadata?: {
    filePath?: string;
    pageNumber?: number;
    snippet?: string;
    lineRange?: [number, number];
  };
}

type ArtifactGroup = {
  key: string;
  label: string;
  icon: React.ElementType;
  items: Artifact[];
};

interface EnhancedArtifactPanelProps {
  artifacts: Artifact[];
  colors?: any;
  theme?: string;
  onClose?: () => void;
  isFullscreen?: boolean;
  onToggleFullscreen?: () => void;
  selectedArtifactId?: string;
}

function getTypeKey(artifactType: any): string {
  if (typeof artifactType === 'string') return artifactType.toLowerCase();
  if (artifactType?.Code) return 'code';
  if (artifactType?.Markdown) return 'markdown';
  if (artifactType?.Mermaid) return 'mermaid';
  if (artifactType?.Html) return 'html';
  if (artifactType?.Svg) return 'svg';
  if (artifactType?.PDF) return 'pdf';
  if (artifactType?.Table) return 'table';
  if (artifactType?.Chart) return 'chart';
  return 'other';
}

function getGroupForType(typeKey: string): string {
  switch (typeKey) {
    case 'chart': return 'charts';
    case 'table': return 'tables';
    case 'mermaid': return 'diagrams';
    case 'code': return 'code';
    case 'pdf':
    case 'markdown':
    case 'html':
    case 'svg':
      return 'documents';
    default: return 'other';
  }
}

const GROUP_CONFIG: Record<string, { label: string; icon: React.ElementType; order: number }> = {
  charts: { label: 'Charts', icon: BarChart, order: 0 },
  tables: { label: 'Tables', icon: Table, order: 1 },
  diagrams: { label: 'Diagrams', icon: ImageIcon, order: 2 },
  code: { label: 'Code', icon: Code, order: 3 },
  documents: { label: 'Documents', icon: FileText, order: 4 },
  other: { label: 'Other', icon: FileCode, order: 5 },
};

function groupArtifacts(artifacts: Artifact[]): ArtifactGroup[] {
  const grouped: Record<string, Artifact[]> = {};

  for (const artifact of artifacts) {
    const typeKey = getTypeKey(artifact.artifact_type);
    const groupKey = getGroupForType(typeKey);
    if (!grouped[groupKey]) grouped[groupKey] = [];
    grouped[groupKey].push(artifact);
  }

  return Object.entries(grouped)
    .map(([key, items]) => ({
      key,
      label: GROUP_CONFIG[key]?.label ?? 'Other',
      icon: GROUP_CONFIG[key]?.icon ?? FileText,
      items,
    }))
    .sort((a, b) => (GROUP_CONFIG[a.key]?.order ?? 99) - (GROUP_CONFIG[b.key]?.order ?? 99));
}

function getFileExtension(artifact: Artifact): string {
  const t = artifact.artifact_type;
  const s = typeof t === 'string' ? t.toLowerCase() : '';
  if (s === 'code' || (t as any).Code) {
    const lang = (t as any).Code?.language || 'txt';
    const extensions: Record<string, string> = {
      javascript: 'js', typescript: 'ts', python: 'py', rust: 'rs',
      java: 'java', cpp: 'cpp', csharp: 'cs', html: 'html',
      css: 'css', json: 'json', yaml: 'yaml', xml: 'xml', markdown: 'md',
    };
    return extensions[lang] || 'txt';
  }
  if (s === 'markdown' || (t as any).Markdown) return 'md';
  if (s === 'mermaid' || (t as any).Mermaid) return 'mmd';
  if (s === 'svg' || (t as any).SVG) return 'svg';
  if (s === 'html' || (t as any).HTML) return 'html';
  return 'txt';
}

function getTypeName(artifact: Artifact): string {
  const t = artifact.artifact_type;
  const s = typeof t === 'string' ? t.toLowerCase() : '';
  if (s === 'code' || (t as any).Code) return 'Code';
  if (s === 'table' || (t as any).Table) return 'Table';
  if (s === 'chart' || (t as any).Chart) return 'Chart';
  if (s === 'markdown' || (t as any).Markdown) return 'Markdown';
  if (s === 'mermaid' || (t as any).Mermaid) return 'Diagram';
  if (s === 'pdf' || (t as any).PDF) return 'PDF';
  if (s === 'svg' || (t as any).SVG) return 'SVG';
  if (s === 'html' || (t as any).HTML) return 'HTML';
  return 'Text';
}

export function EnhancedArtifactPanel({
  artifacts,
  theme: themeProp = 'light',
  onClose,
  isFullscreen = false,
  onToggleFullscreen,
  selectedArtifactId,
}: EnhancedArtifactPanelProps) {
  const { colors, theme } = useTheme();
  const [selectedId, setSelectedId] = useState<string>(selectedArtifactId || artifacts[0]?.id || '');
  const [editMode, setEditMode] = useState(false);
  const [editedContent, setEditedContent] = useState('');
  const [saving, setSaving] = useState(false);
  const [copied, setCopied] = useState(false);
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(new Set());

  const selectedArtifact = artifacts.find(a => a.id === selectedId) || artifacts[0];
  const groups = groupArtifacts(artifacts);

  useEffect(() => {
    if (selectedArtifactId) {
      setSelectedId(selectedArtifactId);
    }
  }, [selectedArtifactId]);

  useEffect(() => {
    if (selectedArtifact) {
      setEditedContent(selectedArtifact.content);
    }
  }, [selectedArtifact]);

  const toggleGroup = (groupKey: string) => {
    setCollapsedGroups(prev => {
      const next = new Set(prev);
      if (next.has(groupKey)) next.delete(groupKey);
      else next.add(groupKey);
      return next;
    });
  };

  const handleCopy = async () => {
    await navigator.clipboard.writeText(selectedArtifact?.content || '');
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  const handleSave = async () => {
    if (!selectedArtifact) return;
    setSaving(true);
    try {
      const updated = await invoke<Artifact>('update_artifact', {
        artifactId: selectedArtifact.id,
        newContent: editedContent,
      });
      setSelectedId(updated.id);
      setEditMode(false);
    } catch (err) {
      console.error('Failed to save artifact:', err);
    } finally {
      setSaving(false);
    }
  };

  const handleDownload = async () => {
    if (!selectedArtifact) return;
    try {
      const ext = getFileExtension(selectedArtifact);
      const defaultPath = `${selectedArtifact.title}.${ext}`;
      const filePath = await save({
        defaultPath,
        filters: [{ name: getTypeName(selectedArtifact), extensions: [ext] }],
      });
      if (filePath) {
        const encoder = new TextEncoder();
        await writeFile(filePath, encoder.encode(selectedArtifact.content));
      }
    } catch (err) {
      console.error('Failed to download artifact:', err);
    }
  };

  if (artifacts.length === 0) {
    return (
      <div className="flex items-center justify-center h-64" style={{ color: colors.textMuted }}>
        No artifacts yet
      </div>
    );
  }

  const isType = (typeName: string) => {
    if (!selectedArtifact) return false;
    const t = selectedArtifact.artifact_type;
    const s = typeof t === 'string' ? t.toLowerCase() : '';
    return s === typeName.toLowerCase() || (t as any)[typeName] !== undefined;
  };

  return (
    <div
      className={`h-full flex flex-col ${isFullscreen ? 'fixed inset-0 z-50' : ''}`}
      style={{ backgroundColor: colors.bg }}
    >
      {/* Header */}
      <div
        className="flex items-center justify-between px-4 py-3 shrink-0"
        style={{ backgroundColor: colors.bgSecondary, borderBottom: `1px solid ${colors.border}` }}
      >
        <div className="flex items-center gap-2">
          <h2 className="font-semibold text-sm" style={{ color: colors.text }}>
            Artifacts
          </h2>
          <span
            className="text-xs px-2 py-0.5 rounded-full font-medium"
            style={{ backgroundColor: `${colors.primary}18`, color: colors.primary }}
          >
            {artifacts.length}
          </span>
        </div>
        <div className="flex items-center gap-1">
          {onToggleFullscreen && (
            <button
              onClick={onToggleFullscreen}
              className="p-1.5 rounded-md transition-colors"
              style={{ color: colors.textMuted }}
              onMouseEnter={e => (e.currentTarget.style.backgroundColor = colors.bgHover)}
              onMouseLeave={e => (e.currentTarget.style.backgroundColor = 'transparent')}
            >
              {isFullscreen ? <Minimize2 className="w-4 h-4" /> : <Maximize2 className="w-4 h-4" />}
            </button>
          )}
          {onClose && (
            <button
              onClick={onClose}
              className="p-1.5 rounded-md transition-colors"
              style={{ color: colors.textMuted }}
              onMouseEnter={e => (e.currentTarget.style.backgroundColor = colors.bgHover)}
              onMouseLeave={e => (e.currentTarget.style.backgroundColor = 'transparent')}
            >
              <X className="w-4 h-4" />
            </button>
          )}
        </div>
      </div>

      {/* Body: Sidebar + Content */}
      <div className="flex-1 flex overflow-hidden">
        {/* Sidebar */}
        <div
          className="w-[180px] shrink-0 overflow-y-auto"
          style={{ backgroundColor: colors.bgSecondary, borderRight: `1px solid ${colors.border}` }}
        >
          {groups.map(group => {
            const GroupIcon = group.icon;
            const isCollapsed = collapsedGroups.has(group.key);
            return (
              <div key={group.key}>
                {/* Group header */}
                <button
                  onClick={() => toggleGroup(group.key)}
                  className="w-full flex items-center gap-1.5 px-3 py-2 text-xs font-semibold uppercase tracking-wider transition-colors"
                  style={{ color: colors.textMuted }}
                  onMouseEnter={e => (e.currentTarget.style.backgroundColor = colors.bgHover)}
                  onMouseLeave={e => (e.currentTarget.style.backgroundColor = 'transparent')}
                >
                  {isCollapsed
                    ? <ChevronRightIcon className="w-3 h-3" />
                    : <ChevronDown className="w-3 h-3" />
                  }
                  <GroupIcon className="w-3.5 h-3.5" />
                  <span>{group.label}</span>
                  <span className="ml-auto font-normal" style={{ color: colors.textTertiary }}>{group.items.length}</span>
                </button>

                {/* Group items */}
                <AnimatePresence initial={false}>
                  {!isCollapsed && (
                    <motion.div
                      initial={{ height: 0, opacity: 0 }}
                      animate={{ height: 'auto', opacity: 1 }}
                      exit={{ height: 0, opacity: 0 }}
                      transition={{ duration: 0.15 }}
                    >
                      {group.items.map(artifact => {
                        const isSelected = selectedId === artifact.id;
                        return (
                          <button
                            key={artifact.id}
                            onClick={() => {
                              setSelectedId(artifact.id);
                              setEditMode(false);
                            }}
                            className="w-full text-left px-3 py-2 pl-7 text-xs transition-colors truncate"
                            style={{
                              backgroundColor: isSelected ? `${colors.primary}14` : 'transparent',
                              color: isSelected ? colors.primary : colors.textSecondary,
                              fontWeight: isSelected ? 500 : 400,
                            }}
                            onMouseEnter={e => {
                              if (!isSelected) e.currentTarget.style.backgroundColor = colors.bgHover;
                            }}
                            onMouseLeave={e => {
                              if (!isSelected) e.currentTarget.style.backgroundColor = 'transparent';
                            }}
                            title={artifact.title}
                          >
                            {artifact.title}
                          </button>
                        );
                      })}
                    </motion.div>
                  )}
                </AnimatePresence>
              </div>
            );
          })}
        </div>

        {/* Content viewer */}
        <div className="flex-1 flex flex-col overflow-hidden">
          {/* Content header with actions */}
          {selectedArtifact && (
            <div
              className="flex items-center justify-between px-4 py-2 shrink-0"
              style={{ borderBottom: `1px solid ${colors.border}` }}
            >
              <div className="min-w-0">
                <p className="text-sm font-medium truncate" style={{ color: colors.text }}>
                  {selectedArtifact.title}
                </p>
                <p className="text-xs" style={{ color: colors.textMuted }}>
                  {getTypeName(selectedArtifact)} &bull; v{selectedArtifact.version}
                </p>
              </div>
              <div className="flex items-center gap-1 shrink-0">
                <button
                  onClick={handleCopy}
                  className="p-1.5 rounded-md transition-colors"
                  style={{ color: copied ? colors.success : colors.textMuted }}
                  onMouseEnter={e => (e.currentTarget.style.backgroundColor = colors.bgHover)}
                  onMouseLeave={e => (e.currentTarget.style.backgroundColor = 'transparent')}
                  title="Copy"
                >
                  {copied ? <Check className="w-4 h-4" /> : <Copy className="w-4 h-4" />}
                </button>
                <button
                  onClick={handleDownload}
                  className="p-1.5 rounded-md transition-colors"
                  style={{ color: colors.textMuted }}
                  onMouseEnter={e => (e.currentTarget.style.backgroundColor = colors.bgHover)}
                  onMouseLeave={e => (e.currentTarget.style.backgroundColor = 'transparent')}
                  title="Download"
                >
                  <Download className="w-4 h-4" />
                </button>
                {selectedArtifact.editable && !editMode && (
                  <button
                    onClick={() => setEditMode(true)}
                    className="p-1.5 rounded-md transition-colors"
                    style={{ color: colors.textMuted }}
                    onMouseEnter={e => (e.currentTarget.style.backgroundColor = colors.bgHover)}
                    onMouseLeave={e => (e.currentTarget.style.backgroundColor = 'transparent')}
                    title="Edit"
                  >
                    <Edit className="w-4 h-4" />
                  </button>
                )}
              </div>
            </div>
          )}

          {/* Edit mode bar */}
          <AnimatePresence>
            {editMode && (
              <motion.div
                initial={{ opacity: 0, height: 0 }}
                animate={{ opacity: 1, height: 'auto' }}
                exit={{ opacity: 0, height: 0 }}
                className="px-4 py-2 flex items-center justify-between shrink-0"
                style={{
                  backgroundColor: `${colors.primary}0a`,
                  borderBottom: `1px solid ${colors.primary}30`,
                }}
              >
                <p className="text-xs font-medium" style={{ color: colors.primary }}>Editing</p>
                <div className="flex gap-2">
                  <button
                    onClick={() => {
                      setEditMode(false);
                      if (selectedArtifact) setEditedContent(selectedArtifact.content);
                    }}
                    className="px-2 py-1 text-xs rounded transition-colors"
                    style={{ border: `1px solid ${colors.border}`, color: colors.text }}
                    onMouseEnter={e => (e.currentTarget.style.backgroundColor = colors.bgHover)}
                    onMouseLeave={e => (e.currentTarget.style.backgroundColor = 'transparent')}
                  >
                    Cancel
                  </button>
                  <button
                    onClick={handleSave}
                    disabled={saving}
                    className="px-2 py-1 text-xs rounded transition-colors flex items-center gap-1 disabled:opacity-50"
                    style={{ backgroundColor: colors.primary, color: colors.primaryText }}
                  >
                    <SaveIcon className="w-3 h-3" />
                    {saving ? 'Saving...' : 'Save'}
                  </button>
                </div>
              </motion.div>
            )}
          </AnimatePresence>

          {/* Content area */}
          <div className="flex-1 overflow-auto p-4">
            <AnimatePresence mode="wait">
              {selectedArtifact && (
                <motion.div
                  key={selectedArtifact.id}
                  initial={{ opacity: 0, y: 8 }}
                  animate={{ opacity: 1, y: 0 }}
                  exit={{ opacity: 0, y: -8 }}
                  transition={{ duration: 0.15 }}
                  className="h-full"
                >
                  {editMode ? (
                    <textarea
                      value={editedContent}
                      onChange={(e) => setEditedContent(e.target.value)}
                      className="w-full h-full min-h-[400px] p-4 font-mono text-sm rounded-lg focus:outline-none"
                      style={{
                        backgroundColor: colors.bgTertiary,
                        border: `1px solid ${colors.border}`,
                        color: colors.text,
                      }}
                    />
                  ) : (
                    <>
                      {isType('Code') && <CodeArtifact artifact={selectedArtifact} theme={theme} />}
                      {isType('Markdown') && <MarkdownArtifact artifact={selectedArtifact} />}
                      {isType('Mermaid') && <MermaidArtifact artifact={selectedArtifact} />}
                      {isType('PDF') && <PDFArtifact artifact={selectedArtifact} />}
                      {isType('Table') && <TableArtifact artifact={selectedArtifact} theme={theme} />}
                      {isType('Chart') && <ChartArtifact artifact={selectedArtifact} theme={theme} />}
                    </>
                  )}
                </motion.div>
              )}
            </AnimatePresence>
          </div>
        </div>
      </div>
    </div>
  );
}

export default EnhancedArtifactPanel;
