import React, { useState, useEffect } from 'react';
import { useTheme } from '../contexts/ThemeContext';
import { Shield, FileText, FilePlus, FolderOpen, Trash2, Clock, CheckCircle, XCircle, RefreshCw } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { notify } from '../lib/notify';

interface AuditEntry {
  timestamp: string;
  agent_id: string;
  operation: string;
  path: string;
  allowed: boolean;
  result: string;
}

export function PermissionManagement() {
  const { colors } = useTheme();
  const [auditLog, setAuditLog] = useState<AuditEntry[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [sessionPermissions, setSessionPermissions] = useState<Map<string, Set<string>>>(new Map());

  useEffect(() => {
    loadAuditLog();
  }, []);

  const loadAuditLog = async () => {
    setIsLoading(true);
    try {
      const log = await invoke<AuditEntry[]>('get_agent_audit_log');
      setAuditLog(log.reverse()); // Most recent first
    } catch (error) {
      console.error('Failed to load audit log:', error);
      notify.error('Failed to load audit log');
    } finally {
      setIsLoading(false);
    }
  };

  const grantSessionPermission = async (agentId: string, operation: string) => {
    try {
      await invoke('grant_session_permission', {
        agentId,
        operation,
      });

      // Update local state
      setSessionPermissions((prev) => {
        const newMap = new Map(prev);
        if (!newMap.has(agentId)) {
          newMap.set(agentId, new Set());
        }
        newMap.get(agentId)!.add(operation);
        return newMap;
      });

      notify.success(`Session permission granted for ${getOperationTitle(operation)}`);
    } catch (error) {
      console.error('Failed to grant permission:', error);
      notify.error('Failed to grant permission');
    }
  };

  const getOperationTitle = (operation: string) => {
    switch (operation) {
      case 'read_file': return 'Read Files';
      case 'write_file': return 'Write Files';
      case 'list_directory': return 'List Directories';
      case 'delete_file': return 'Delete Files';
      case 'create_directory': return 'Create Directories';
      case 'execute_code': return 'Execute Code';
      default: return operation;
    }
  };

  const getOperationIcon = (operation: string) => {
    switch (operation) {
      case 'read_file': return FileText;
      case 'write_file': return FilePlus;
      case 'list_directory': return FolderOpen;
      case 'delete_file': return Trash2;
      case 'create_directory': return FolderOpen;
      case 'execute_code': return Shield;
      default: return Shield;
    }
  };

  const getOperationColor = (operation: string) => {
    switch (operation) {
      case 'read_file': return colors.info;
      case 'write_file': return colors.warning;
      case 'list_directory': return colors.info;
      case 'delete_file': return colors.error;
      case 'create_directory': return colors.warning;
      case 'execute_code': return colors.error;
      default: return colors.textMuted;
    }
  };

  const formatTimestamp = (timestamp: string) => {
    const date = new Date(timestamp);
    const now = new Date();
    const diffMs = now.getTime() - date.getTime();
    const diffMins = Math.floor(diffMs / 60000);

    if (diffMins < 1) return 'Just now';
    if (diffMins < 60) return `${diffMins}m ago`;
    if (diffMins < 1440) return `${Math.floor(diffMins / 60)}h ago`;
    return date.toLocaleDateString() + ' ' + date.toLocaleTimeString();
  };

  const permissions = [
    { id: 'read_file', name: 'Read Files', description: 'Allow reading file contents', icon: FileText },
    { id: 'write_file', name: 'Write Files', description: 'Allow creating/modifying files', icon: FilePlus },
    { id: 'list_directory', name: 'List Directories', description: 'Allow viewing directory contents', icon: FolderOpen },
    { id: 'delete_file', name: 'Delete Files', description: 'Allow deleting files', icon: Trash2, dangerous: true },
    { id: 'create_directory', name: 'Create Directories', description: 'Allow creating new directories', icon: FolderOpen },
    { id: 'execute_code', name: 'Execute Code', description: 'Allow running code (temp files)', icon: Shield, dangerous: true },
  ];

  return (
    <div className="h-full flex flex-col" style={{ background: colors.bg }}>
      {/* Header */}
      <div className="p-6 border-b" style={{ borderColor: colors.border }}>
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-lg" style={{ background: colors.primary + '20' }}>
              <Shield className="w-6 h-6" style={{ color: colors.primary }} />
            </div>
            <div>
              <h2 className="text-2xl font-bold" style={{ color: colors.text }}>
                Permission Management
              </h2>
              <p className="text-sm" style={{ color: colors.textMuted }}>
                Manage agent file system permissions and view audit log
              </p>
            </div>
          </div>
          <button
            onClick={loadAuditLog}
            className="flex items-center gap-2 px-4 py-2 rounded-lg border-2 transition-all hover:opacity-80"
            style={{
              background: colors.bgSecondary,
              borderColor: colors.border,
              color: colors.text,
            }}
          >
            <RefreshCw className="w-4 h-4" />
            Refresh
          </button>
        </div>
      </div>

      <div className="flex-1 overflow-y-auto p-6 space-y-6">
        {/* Quick Grant Permissions */}
        <div>
          <h3 className="text-lg font-bold mb-3" style={{ color: colors.text }}>
            Quick Grant Session Permissions
          </h3>
          <p className="text-sm mb-4" style={{ color: colors.textMuted }}>
            Pre-approve operations for agents during this session
          </p>
          <div className="grid grid-cols-2 gap-3">
            {permissions.map((permission) => {
              const Icon = permission.icon;
              const hasPermission = sessionPermissions.get('default')?.has(permission.id);

              return (
                <button
                  key={permission.id}
                  onClick={() => !hasPermission && grantSessionPermission('default', permission.id)}
                  className="flex items-start gap-3 p-4 rounded-lg border-2 transition-all text-left"
                  style={{
                    background: hasPermission ? colors.success + '20' : colors.bgSecondary,
                    borderColor: hasPermission
                      ? colors.success
                      : permission.dangerous
                      ? colors.error
                      : colors.border,
                    opacity: hasPermission ? 0.7 : 1,
                    cursor: hasPermission ? 'default' : 'pointer',
                  }}
                  disabled={hasPermission}
                >
                  <div className="p-2 rounded-lg" style={{
                    background: getOperationColor(permission.id) + '20'
                  }}>
                    <Icon className="w-5 h-5" style={{ color: getOperationColor(permission.id) }} />
                  </div>
                  <div className="flex-1">
                    <div className="flex items-center gap-2 mb-1">
                      <p className="font-semibold" style={{ color: colors.text }}>
                        {permission.name}
                      </p>
                      {hasPermission && (
                        <CheckCircle className="w-4 h-4" style={{ color: colors.success }} />
                      )}
                    </div>
                    <p className="text-xs" style={{ color: colors.textMuted }}>
                      {permission.description}
                    </p>
                  </div>
                </button>
              );
            })}
          </div>
        </div>

        {/* Audit Log */}
        <div>
          <div className="flex items-center justify-between mb-3">
            <h3 className="text-lg font-bold" style={{ color: colors.text }}>
              Audit Log
            </h3>
            <span className="text-sm px-3 py-1 rounded-full" style={{
              background: colors.bgTertiary,
              color: colors.textMuted,
            }}>
              {auditLog.length} entries
            </span>
          </div>

          {isLoading ? (
            <div className="text-center py-12" style={{ color: colors.textMuted }}>
              Loading audit log...
            </div>
          ) : auditLog.length === 0 ? (
            <div className="text-center py-12 rounded-lg border-2 border-dashed" style={{
              borderColor: colors.border,
              color: colors.textMuted,
            }}>
              <Shield className="w-12 h-12 mx-auto mb-3 opacity-50" />
              <p>No file operations yet</p>
              <p className="text-sm mt-1">Agent file operations will appear here</p>
            </div>
          ) : (
            <div className="space-y-2">
              {auditLog.map((entry, index) => {
                const Icon = getOperationIcon(entry.operation);
                const operationColor = getOperationColor(entry.operation);

                return (
                  <div
                    key={index}
                    className="flex items-start gap-3 p-4 rounded-lg border"
                    style={{
                      background: colors.bgSecondary,
                      borderColor: entry.allowed ? colors.border : colors.error + '40',
                    }}
                  >
                    <div className="p-2 rounded-lg" style={{ background: operationColor + '20' }}>
                      <Icon className="w-4 h-4" style={{ color: operationColor }} />
                    </div>

                    <div className="flex-1 min-w-0">
                      <div className="flex items-center gap-2 mb-1">
                        <p className="font-semibold text-sm" style={{ color: colors.text }}>
                          {getOperationTitle(entry.operation)}
                        </p>
                        {entry.allowed ? (
                          <CheckCircle className="w-4 h-4" style={{ color: colors.success }} />
                        ) : (
                          <XCircle className="w-4 h-4" style={{ color: colors.error }} />
                        )}
                      </div>

                      <p className="text-xs font-mono truncate mb-1" style={{
                        color: colors.textMuted,
                      }}>
                        {entry.path}
                      </p>

                      <div className="flex items-center gap-3 text-xs" style={{ color: colors.textMuted }}>
                        <span className="flex items-center gap-1">
                          <Shield className="w-3 h-3" />
                          {entry.agent_id}
                        </span>
                        <span className="flex items-center gap-1">
                          <Clock className="w-3 h-3" />
                          {formatTimestamp(entry.timestamp)}
                        </span>
                      </div>

                      {entry.result && (
                        <p className="text-xs mt-2 p-2 rounded" style={{
                          background: colors.bgTertiary,
                          color: colors.textMuted,
                        }}>
                          {entry.result}
                        </p>
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
