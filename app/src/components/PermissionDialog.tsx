import React, { useState } from 'react';
import { useTheme } from '../contexts/ThemeContext';
import { Shield, FileText, FilePlus, FolderOpen, Trash2, X, AlertTriangle, Lock } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { notify } from '../lib/notify';

export interface PermissionRequest {
  operation: string;  // "read_file", "write_file", "list_directory", "delete_file", etc.
  path: string;
  reason: string;
  agent_id: string;
  agent_name?: string;
}

interface PermissionDialogProps {
  isOpen: boolean;
  request: PermissionRequest | null;
  onDecision: (allowed: boolean, scope: 'once' | 'session') => void;
  onClose: () => void;
}

export function PermissionDialog({ isOpen, request, onDecision, onClose }: PermissionDialogProps) {
  const { colors } = useTheme();
  const [isProcessing, setIsProcessing] = useState(false);

  if (!isOpen || !request) return null;

  const getOperationInfo = (operation: string) => {
    switch (operation) {
      case 'read_file':
        return {
          icon: FileText,
          title: 'Read File',
          color: colors.info,
          description: 'wants to read the contents of a file',
        };
      case 'write_file':
        return {
          icon: FilePlus,
          title: 'Write File',
          color: colors.warning,
          description: 'wants to create or modify a file',
        };
      case 'list_directory':
        return {
          icon: FolderOpen,
          title: 'List Directory',
          color: colors.info,
          description: 'wants to view the contents of a directory',
        };
      case 'delete_file':
        return {
          icon: Trash2,
          title: 'Delete File',
          color: colors.error,
          description: 'wants to delete a file',
        };
      case 'create_directory':
        return {
          icon: FolderOpen,
          title: 'Create Directory',
          color: colors.warning,
          description: 'wants to create a new directory',
        };
      case 'execute_code':
        return {
          icon: AlertTriangle,
          title: 'Execute Code',
          color: colors.error,
          description: 'wants to execute code (writes to temp directory)',
        };
      default:
        return {
          icon: Shield,
          title: 'File Operation',
          color: colors.primary,
          description: 'wants to perform a file operation',
        };
    }
  };

  const handleDecision = async (allowed: boolean, scope: 'once' | 'session') => {
    setIsProcessing(true);

    try {
      if (allowed && scope === 'session') {
        // Grant session-level permission
        await invoke('grant_session_permission', {
          agentId: request.agent_id,
          operation: request.operation,
        });
        notify.success(`Session permission granted for ${getOperationInfo(request.operation).title}`);
      }

      // Call the decision callback
      onDecision(allowed, scope);
    } catch (error) {
      console.error('Failed to grant permission:', error);
      notify.error('Failed to process permission request');
    } finally {
      setIsProcessing(false);
    }
  };

  const operationInfo = getOperationInfo(request.operation);
  const OperationIcon = operationInfo.icon;
  const agentName = request.agent_name || request.agent_id;
  const isDangerous = request.operation === 'delete_file' || request.operation === 'execute_code' || request.operation === 'write_file';

  return (
    <>
      {/* Backdrop */}
      <div
        className="fixed inset-0 z-50"
        style={{ background: 'rgba(0, 0, 0, 0.6)' }}
        onClick={isProcessing ? undefined : () => handleDecision(false, 'once')}
      />

      {/* Dialog */}
      <div className="fixed top-1/2 left-1/2 transform -translate-x-1/2 -translate-y-1/2 z-50 w-full max-w-2xl">
        <div className="rounded-xl shadow-2xl border-2 overflow-hidden" style={{ background: colors.bg, borderColor: colors.border }}>

          {/* Header */}
          <div className="flex items-center justify-between p-6 border-b" style={{
            background: isDangerous ? `${operationInfo.color}20` : colors.bgSecondary,
            borderColor: colors.border
          }}>
            <div className="flex items-center gap-3">
              <div className="p-2 rounded-lg" style={{ background: `${operationInfo.color}30` }}>
                <OperationIcon className="w-6 h-6" style={{ color: operationInfo.color }} />
              </div>
              <div>
                <h2 className="text-xl font-bold" style={{ color: colors.text }}>
                  Permission Request
                </h2>
                <p className="text-sm" style={{ color: colors.textMuted }}>
                  {operationInfo.title}
                </p>
              </div>
            </div>
            <button
              onClick={() => !isProcessing && handleDecision(false, 'once')}
              className="p-2 rounded-lg transition-all hover:opacity-70"
              style={{ background: colors.bgTertiary }}
              disabled={isProcessing}
            >
              <X className="w-5 h-5" style={{ color: colors.text }} />
            </button>
          </div>

          {/* Content */}
          <div className="p-6 space-y-5">
            {/* Agent Info */}
            <div className="flex items-start gap-3 p-4 rounded-lg" style={{ background: colors.bgSecondary }}>
              <Shield className="w-5 h-5 mt-0.5" style={{ color: colors.primary }} />
              <div className="flex-1">
                <p className="text-sm font-medium mb-1" style={{ color: colors.text }}>
                  Agent: <span className="font-bold">{agentName}</span>
                </p>
                <p className="text-sm" style={{ color: colors.textMuted }}>
                  {operationInfo.description}
                </p>
              </div>
            </div>

            {/* Path */}
            <div>
              <label className="block text-xs font-semibold mb-2 uppercase tracking-wide" style={{ color: colors.textMuted }}>
                File Path
              </label>
              <div className="p-3 rounded-lg font-mono text-sm break-all" style={{
                background: colors.bgTertiary,
                color: colors.text,
                border: `1px solid ${colors.border}`,
              }}>
                {request.path}
              </div>
            </div>

            {/* Reason */}
            {request.reason && (
              <div>
                <label className="block text-xs font-semibold mb-2 uppercase tracking-wide" style={{ color: colors.textMuted }}>
                  Reason
                </label>
                <div className="p-3 rounded-lg text-sm" style={{
                  background: colors.bgTertiary,
                  color: colors.text,
                  border: `1px solid ${colors.border}`,
                }}>
                  {request.reason}
                </div>
              </div>
            )}

            {/* Warning for dangerous operations */}
            {isDangerous && (
              <div className="flex items-start gap-3 p-4 rounded-lg border-2" style={{
                background: `${colors.warning}10`,
                borderColor: colors.warning,
              }}>
                <AlertTriangle className="w-5 h-5 mt-0.5" style={{ color: colors.warning }} />
                <div>
                  <p className="text-sm font-semibold mb-1" style={{ color: colors.text }}>
                    Caution Required
                  </p>
                  <p className="text-xs" style={{ color: colors.textMuted }}>
                    {request.operation === 'delete_file'
                      ? 'This will permanently delete the file from your system.'
                      : request.operation === 'execute_code'
                      ? 'Code execution can pose security risks. Only allow trusted agents.'
                      : 'This will modify files on your system. Make sure you trust this agent.'}
                  </p>
                </div>
              </div>
            )}

            {/* Session Permission Info */}
            <div className="flex items-start gap-3 p-4 rounded-lg border" style={{
              background: colors.bgSecondary,
              borderColor: colors.border,
            }}>
              <Lock className="w-4 h-4 mt-0.5" style={{ color: colors.textMuted }} />
              <p className="text-xs" style={{ color: colors.textMuted }}>
                <strong>Session Permission:</strong> Approving for the session will allow this agent to perform this operation
                for the rest of your session without asking again.
              </p>
            </div>
          </div>

          {/* Actions */}
          <div className="flex gap-3 p-6 border-t" style={{
            background: colors.bgSecondary,
            borderColor: colors.border,
          }}>
            <button
              onClick={() => handleDecision(false, 'once')}
              className="flex-1 py-3 px-6 rounded-lg border-2 font-semibold transition-all hover:opacity-80"
              style={{
                background: colors.bgTertiary,
                borderColor: colors.border,
                color: colors.text,
              }}
              disabled={isProcessing}
            >
              Deny
            </button>

            <button
              onClick={() => handleDecision(true, 'once')}
              className="flex-1 py-3 px-6 rounded-lg border-2 font-semibold transition-all hover:opacity-90"
              style={{
                background: colors.bgTertiary,
                borderColor: colors.primary,
                color: colors.primary,
              }}
              disabled={isProcessing}
            >
              Allow Once
            </button>

            <button
              onClick={() => handleDecision(true, 'session')}
              className="flex-1 py-3 px-6 rounded-lg border-2 font-semibold transition-all hover:opacity-90 shadow-lg"
              style={{
                background: colors.primary,
                borderColor: colors.primary,
                color: '#ffffff',
              }}
              disabled={isProcessing}
            >
              Allow for Session
            </button>
          </div>
        </div>
      </div>
    </>
  );
}
