import React, { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Database, Trash2, FolderOpen, RefreshCw, AlertTriangle, HardDrive, FileText } from 'lucide-react';
import { useTheme } from '../contexts/ThemeContext';
import { notify } from '../lib/notify';

interface IndexStats {
  total_spaces: number;
  total_documents: number;
  total_vectors: number;
  database_size_mb: number;
}

interface Source {
  id: string;
  name: string;
  path: string;
  fileCount: number;
  status: string;
}

interface IndexedSource {
  doc_id: string;
  title: string;
  source: string;
}

interface DataManagementProps {
  sources: Source[];
  onRemoveSource: (id: string, event: React.MouseEvent) => void;
  onSourcesCleared: () => void;
}

export default function DataManagement({ sources, onRemoveSource, onSourcesCleared }: DataManagementProps) {
  const { colors } = useTheme();
  const [stats, setStats] = useState<IndexStats | null>(null);
  const [indexedSources, setIndexedSources] = useState<IndexedSource[]>([]);
  const [showIndexed, setShowIndexed] = useState(false);
  const [loading, setLoading] = useState(false);
  const [confirmAction, setConfirmAction] = useState<'clear_docs' | 'reset' | null>(null);

  const loadStats = async () => {
    try {
      const dbStats = await invoke<IndexStats>('get_database_stats');
      setStats(dbStats);
    } catch (err) {
      console.error('Failed to load database stats:', err);
    }
  };

  const loadIndexedSources = async () => {
    try {
      const sources = await invoke<IndexedSource[]>('list_indexed_sources');
      setIndexedSources(sources);
    } catch (err) {
      console.error('Failed to load indexed sources:', err);
    }
  };

  useEffect(() => {
    loadStats();
  }, []);

  const formatSize = (mb: number) => {
    if (mb < 1) return `${(mb * 1024).toFixed(1)} KB`;
    if (mb > 1024) return `${(mb / 1024).toFixed(2)} GB`;
    return `${mb.toFixed(2)} MB`;
  };

  const handleClearAllDocuments = async () => {
    setConfirmAction(null);
    setLoading(true);
    try {
      await invoke('clear_all_documents');
      notify.success('All documents cleared from index');
      onSourcesCleared();
      setIndexedSources([]);
      await loadStats();
    } catch (err) {
      notify.error(`Failed to clear documents: ${err}`);
    } finally {
      setLoading(false);
    }
  };

  const handleCleanupOrphaned = async () => {
    setLoading(true);
    try {
      const result = await invoke<string>('cleanup_orphaned_documents');
      notify.success(result || 'Orphaned documents cleaned up');
      await loadStats();
    } catch (err) {
      notify.error(`Cleanup failed: ${err}`);
    } finally {
      setLoading(false);
    }
  };

  const handleResetDatabase = async () => {
    setConfirmAction(null);
    setLoading(true);
    try {
      await invoke('reset_database');
      notify.success('Database reset. Restart the app for a fresh start.');
      onSourcesCleared();
      setIndexedSources([]);
      await loadStats();
    } catch (err) {
      notify.error(`Reset failed: ${err}`);
    } finally {
      setLoading(false);
    }
  };

  const handleToggleIndexed = async () => {
    if (!showIndexed) {
      await loadIndexedSources();
    }
    setShowIndexed(!showIndexed);
  };

  return (
    <div className="space-y-4">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <Database className="w-4 h-4" style={{ color: colors.primary }} />
          <span className="text-sm font-semibold" style={{ color: colors.text }}>Data Management</span>
        </div>
        <button
          onClick={() => { loadStats(); }}
          className="text-[10px] flex items-center gap-1 px-2 py-1 rounded border transition-colors"
          style={{ borderColor: colors.border, color: colors.textMuted }}
        >
          <RefreshCw className={`w-3 h-3 ${loading ? 'animate-spin' : ''}`} />
          Refresh
        </button>
      </div>

      {/* Index Statistics */}
      {stats && (
        <div className="grid grid-cols-4 gap-2">
          {[
            { label: 'Documents', value: (stats.total_documents ?? 0).toLocaleString() },
            { label: 'Vectors', value: (stats.total_vectors ?? 0).toLocaleString() },
            { label: 'Spaces', value: (stats.total_spaces ?? 0).toLocaleString() },
            { label: 'Size', value: formatSize(stats.database_size_mb ?? 0) },
          ].map(item => (
            <div
              key={item.label}
              className="rounded-md p-2 text-center"
              style={{ backgroundColor: `${colors.primary}08`, border: `1px solid ${colors.border}` }}
            >
              <div className="text-xs font-bold" style={{ color: colors.text }}>{item.value}</div>
              <div className="text-[10px]" style={{ color: colors.textMuted }}>{item.label}</div>
            </div>
          ))}
        </div>
      )}

      {/* Indexed Sources from frontend state */}
      {sources.length > 0 && (
        <div>
          <label className="text-xs font-medium block mb-1.5" style={{ color: colors.textSecondary }}>
            Indexed Sources ({sources.length})
          </label>
          <div className="space-y-1 max-h-40 overflow-y-auto pr-1">
            {sources.map(source => (
              <div
                key={source.id}
                className="flex items-center justify-between rounded-md px-2.5 py-1.5 group"
                style={{ backgroundColor: `${colors.primary}06`, border: `1px solid ${colors.border}` }}
              >
                <div className="flex items-center gap-2 min-w-0">
                  <FolderOpen className="w-3 h-3 shrink-0" style={{ color: colors.textMuted }} />
                  <div className="min-w-0">
                    <div className="text-[11px] font-medium truncate" style={{ color: colors.text }}>
                      {source.name}
                    </div>
                    <div className="text-[10px] truncate" style={{ color: colors.textMuted }}>
                      {source.path}
                    </div>
                  </div>
                </div>
                <button
                  onClick={(e) => onRemoveSource(source.id, e)}
                  className="p-1 rounded opacity-0 group-hover:opacity-100 transition-opacity hover:bg-red-500/10"
                  title="Remove from index"
                >
                  <Trash2 className="w-3 h-3 text-red-500" />
                </button>
              </div>
            ))}
          </div>
        </div>
      )}

      {sources.length === 0 && (
        <div className="text-center py-3 rounded-md" style={{ backgroundColor: `${colors.primary}06`, border: `1px solid ${colors.border}` }}>
          <HardDrive className="w-5 h-5 mx-auto mb-1" style={{ color: colors.textMuted }} />
          <p className="text-[11px]" style={{ color: colors.textMuted }}>No indexed sources</p>
        </div>
      )}

      {/* Actual indexed documents from vector store */}
      <div>
        <button
          onClick={handleToggleIndexed}
          className="w-full text-left text-[11px] px-3 py-2 rounded-md border transition-colors flex items-center gap-2"
          style={{ borderColor: colors.border, color: colors.text }}
        >
          <FileText className="w-3 h-3" style={{ color: colors.primary }} />
          {showIndexed ? 'Hide' : 'Show'} Actual Indexed Files
          {indexedSources.length > 0 && (
            <span className="ml-auto text-[10px]" style={{ color: colors.textMuted }}>
              {indexedSources.length} files
            </span>
          )}
        </button>

        {showIndexed && (
          <div className="mt-1.5 space-y-1 max-h-60 overflow-y-auto pr-1">
            {indexedSources.length === 0 ? (
              <div className="text-[11px] text-center py-2" style={{ color: colors.textMuted }}>
                No files found in vector store
              </div>
            ) : (
              indexedSources.map((src, i) => (
                <div
                  key={src.doc_id + i}
                  className="rounded-md px-2.5 py-1.5"
                  style={{ backgroundColor: `${colors.primary}06`, border: `1px solid ${colors.border}` }}
                >
                  <div className="text-[11px] font-medium truncate" style={{ color: colors.text }}>
                    {src.title}
                  </div>
                  <div className="text-[10px] truncate" style={{ color: colors.textMuted }}>
                    {src.source}
                  </div>
                </div>
              ))
            )}
          </div>
        )}
      </div>

      {/* Actions */}
      <div className="space-y-1.5">
        <label className="text-xs font-medium block mb-1" style={{ color: colors.textSecondary }}>
          Maintenance
        </label>

        <button
          onClick={handleCleanupOrphaned}
          disabled={loading}
          className="w-full text-left text-[11px] px-3 py-2 rounded-md border transition-colors flex items-center gap-2"
          style={{
            borderColor: colors.border,
            color: colors.text,
            opacity: loading ? 0.5 : 1,
          }}
        >
          <RefreshCw className="w-3 h-3" style={{ color: colors.primary }} />
          Clean Up Orphaned Documents
          <span className="ml-auto text-[10px]" style={{ color: colors.textMuted }}>Safe</span>
        </button>

        <button
          onClick={() => setConfirmAction('clear_docs')}
          disabled={loading}
          className="w-full text-left text-[11px] px-3 py-2 rounded-md border transition-colors flex items-center gap-2"
          style={{
            borderColor: colors.border,
            color: colors.text,
            opacity: loading ? 0.5 : 1,
          }}
        >
          <Trash2 className="w-3 h-3 text-orange-500" />
          Clear All Documents
          <span className="ml-auto text-[10px] text-orange-500">Keeps spaces</span>
        </button>

        <button
          onClick={() => setConfirmAction('reset')}
          disabled={loading}
          className="w-full text-left text-[11px] px-3 py-2 rounded-md border transition-colors flex items-center gap-2"
          style={{
            borderColor: colors.border,
            color: colors.text,
            opacity: loading ? 0.5 : 1,
          }}
        >
          <AlertTriangle className="w-3 h-3 text-red-500" />
          Reset Entire Database
          <span className="ml-auto text-[10px] text-red-500">Destructive</span>
        </button>
      </div>

      {/* Confirmation Inline */}
      {confirmAction && (
        <div
          className="rounded-md p-3 space-y-2"
          style={{ backgroundColor: '#fef2f2', border: '1px solid #fca5a5' }}
        >
          <p className="text-[11px] font-medium text-red-800">
            {confirmAction === 'clear_docs'
              ? 'This will permanently delete all indexed documents from the vector store and text index. Spaces will be preserved. This cannot be undone.'
              : 'This will delete everything — all documents, vectors, and spaces. The app will need a restart. This cannot be undone.'}
          </p>
          <div className="flex gap-2">
            <button
              onClick={() => setConfirmAction(null)}
              className="text-[11px] px-3 py-1 rounded border border-gray-300 text-gray-700 hover:bg-gray-100"
            >
              Cancel
            </button>
            <button
              onClick={confirmAction === 'clear_docs' ? handleClearAllDocuments : handleResetDatabase}
              className="text-[11px] px-3 py-1 rounded bg-red-600 text-white hover:bg-red-700"
            >
              {confirmAction === 'clear_docs' ? 'Clear All Documents' : 'Reset Database'}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
