import React, { useState, useEffect } from 'react';
import { useTheme } from '../contexts/ThemeContext';
import { invoke } from '@tauri-apps/api/core';
import { notify } from '../lib/notify';
import {
  Download,
  Upload,
  Trash2,
  Clock,
  HardDrive,
  X,
  RefreshCw,
  Database,
  AlertCircle,
} from 'lucide-react';
import { Button } from './ui/button';
import { createBackup, restoreFromBackup, getLastBackupDate } from '../lib/database';
import { formatBytes } from '../lib/utils';

interface BackupInfo {
  file_name: string;
  file_path: string;
  size_bytes: number;
  created_at: number;
}

interface BackupManagerProps {
  isOpen: boolean;
  onClose: () => void;
}

export function BackupManager({ isOpen, onClose }: BackupManagerProps) {
  const { colors } = useTheme();
  const [backups, setBackups] = useState<BackupInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [creating, setCreating] = useState(false);
  const [lastBackup, setLastBackup] = useState<Date | null>(null);

  useEffect(() => {
    if (isOpen) {
      loadBackups();
      setLastBackup(getLastBackupDate());
    }
  }, [isOpen]);

  async function loadBackups() {
    setLoading(true);
    try {
      const backupList = await invoke<BackupInfo[]>('list_backup_files');
      setBackups(backupList);
    } catch (error) {
      notify.error(`Failed to load backups: ${(error as Error).message}`);
    } finally {
      setLoading(false);
    }
  }

  async function handleCreateBackup() {
    setCreating(true);
    try {
      const backupPath = await createBackup('manual');
      if (backupPath) {
        notify.success('Backup created successfully');
        await loadBackups();
        setLastBackup(new Date());
      }
    } catch (error) {
      notify.error(`Failed to create backup: ${(error as Error).message}`);
    } finally {
      setCreating(false);
    }
  }

  async function handleRestore(backupPath: string, fileName: string) {
    const confirmed = window.confirm(
      `Are you sure you want to restore from "${fileName}"?\n\n` +
      'This will:\n' +
      '1. Create a safety backup of current data\n' +
      '2. Restore all workspaces from the backup\n' +
      '3. Restart the application\n\n' +
      'This action cannot be undone.'
    );

    if (!confirmed) return;

    try {
      const success = await restoreFromBackup(backupPath);
      if (success) {
        notify.success('Backup restored. Please restart the application.', { duration: 10000 });
        setTimeout(() => {
          window.location.reload();
        }, 3000);
      }
    } catch (error) {
      notify.error(`Failed to restore backup: ${(error as Error).message}`);
    }
  }

  function formatDate(timestamp: number): string {
    return new Date(timestamp * 1000).toLocaleString();
  }

  function getTimeSinceBackup(): string {
    if (!lastBackup) return 'Never';

    const diffMs = Date.now() - lastBackup.getTime();
    const diffDays = Math.floor(diffMs / (1000 * 60 * 60 * 24));

    if (diffDays === 0) {
      const diffHours = Math.floor(diffMs / (1000 * 60 * 60));
      if (diffHours === 0) {
        const diffMinutes = Math.floor(diffMs / (1000 * 60));
        return `${diffMinutes} minute${diffMinutes !== 1 ? 's' : ''} ago`;
      }
      return `${diffHours} hour${diffHours !== 1 ? 's' : ''} ago`;
    }

    return `${diffDays} day${diffDays !== 1 ? 's' : ''} ago`;
  }

  if (!isOpen) return null;

  return (
    <>
      <div className="fixed inset-0 z-50" style={{ background: 'rgba(0, 0, 0, 0.5)' }} onClick={onClose} />

      <div className="fixed top-1/2 left-1/2 transform -translate-x-1/2 -translate-y-1/2 z-50 w-full max-w-3xl">
        <div className="rounded-xl shadow-2xl border-2" style={{ background: colors.bg, borderColor: colors.border }}>
          {/* Header */}
          <div className="flex items-center justify-between p-6 border-b" style={{ background: colors.bgSecondary, borderColor: colors.border }}>
            <div className="flex items-center gap-3">
              <Database className="w-6 h-6" style={{ color: colors.primary }} />
              <h2 className="text-xl font-bold" style={{ color: colors.text }}>
                Backup & Restore
              </h2>
            </div>
            <button
              onClick={onClose}
              className="p-2 rounded-lg transition-all hover:opacity-70"
              style={{ background: colors.bgTertiary }}
            >
              <X className="w-5 h-5" style={{ color: colors.text }} />
            </button>
          </div>

          {/* Stats */}
          <div className="p-6 border-b" style={{ borderColor: colors.border }}>
            <div className="grid grid-cols-3 gap-4">
              <div className="p-4 rounded-lg" style={{ background: colors.bgSecondary }}>
                <div className="flex items-center gap-2 mb-2">
                  <Clock className="w-4 h-4" style={{ color: colors.textMuted }} />
                  <span className="text-xs" style={{ color: colors.textMuted }}>Last Backup</span>
                </div>
                <p className="text-sm font-semibold" style={{ color: colors.text }}>
                  {getTimeSinceBackup()}
                </p>
              </div>

              <div className="p-4 rounded-lg" style={{ background: colors.bgSecondary }}>
                <div className="flex items-center gap-2 mb-2">
                  <HardDrive className="w-4 h-4" style={{ color: colors.textMuted }} />
                  <span className="text-xs" style={{ color: colors.textMuted }}>Total Backups</span>
                </div>
                <p className="text-sm font-semibold" style={{ color: colors.text }}>
                  {backups.length}
                </p>
              </div>

              <div className="p-4 rounded-lg" style={{ background: colors.bgSecondary }}>
                <div className="flex items-center gap-2 mb-2">
                  <Download className="w-4 h-4" style={{ color: colors.textMuted }} />
                  <span className="text-xs" style={{ color: colors.textMuted }}>Total Size</span>
                </div>
                <p className="text-sm font-semibold" style={{ color: colors.text }}>
                  {formatBytes(backups.reduce((sum, b) => sum + b.size_bytes, 0))}
                </p>
              </div>
            </div>
          </div>

          {/* Actions */}
          <div className="p-6 border-b flex items-center justify-between" style={{ borderColor: colors.border }}>
            <div className="flex items-start gap-2">
              <AlertCircle className="w-5 h-5 mt-0.5" style={{ color: colors.warning }} />
              <div>
                <p className="text-sm font-medium" style={{ color: colors.text }}>
                  Automatic Weekly Backups Enabled
                </p>
                <p className="text-xs mt-1" style={{ color: colors.textMuted }}>
                  Backups are created automatically every 7 days
                </p>
              </div>
            </div>

            <div className="flex gap-2">
              <Button
                variant="outline"
                size="sm"
                onClick={loadBackups}
                disabled={loading}
              >
                <RefreshCw className={`w-4 h-4 mr-2 ${loading ? 'animate-spin' : ''}`} />
                Refresh
              </Button>
              <Button
                variant="default"
                size="sm"
                onClick={handleCreateBackup}
                disabled={creating}
              >
                <Download className="w-4 h-4 mr-2" />
                {creating ? 'Creating...' : 'Create Backup'}
              </Button>
            </div>
          </div>

          {/* Backup List */}
          <div className="p-6 max-h-96 overflow-y-auto">
            {loading ? (
              <div className="text-center py-8" style={{ color: colors.textMuted }}>
                <RefreshCw className="w-8 h-8 mx-auto mb-2 animate-spin" />
                <p>Loading backups...</p>
              </div>
            ) : backups.length === 0 ? (
              <div className="text-center py-8" style={{ color: colors.textMuted }}>
                <Database className="w-12 h-12 mx-auto mb-3" style={{ color: colors.textMuted }} />
                <p className="font-medium mb-1">No backups found</p>
                <p className="text-sm">Create your first backup to protect your data</p>
              </div>
            ) : (
              <div className="space-y-3">
                {backups.map((backup) => (
                  <div
                    key={backup.file_path}
                    className="p-4 rounded-lg border-2 flex items-center justify-between hover:scale-[1.02] transition-all"
                    style={{ background: colors.bgSecondary, borderColor: colors.border }}
                  >
                    <div className="flex items-center gap-3 flex-1">
                      <Database className="w-5 h-5" style={{ color: colors.primary }} />
                      <div className="flex-1">
                        <p className="font-medium text-sm mb-1" style={{ color: colors.text }}>
                          {backup.file_name}
                        </p>
                        <div className="flex items-center gap-4 text-xs" style={{ color: colors.textMuted }}>
                          <span className="flex items-center gap-1">
                            <Clock className="w-3 h-3" />
                            {formatDate(backup.created_at)}
                          </span>
                          <span className="flex items-center gap-1">
                            <HardDrive className="w-3 h-3" />
                            {formatBytes(backup.size_bytes)}
                          </span>
                        </div>
                      </div>
                    </div>

                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => handleRestore(backup.file_path, backup.file_name)}
                    >
                      <Upload className="w-4 h-4 mr-2" />
                      Restore
                    </Button>
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>
      </div>
    </>
  );
}
