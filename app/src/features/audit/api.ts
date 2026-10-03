import { invoke } from '@tauri-apps/api/core';
import type { AuditPage, AuditQuery, AuditStats, VerifyReport } from './types';

export function queryAudit(query: AuditQuery): Promise<AuditPage> {
  return invoke<AuditPage>('audit_query', { query });
}

export function verifyAudit(): Promise<VerifyReport> {
  return invoke<VerifyReport>('audit_verify');
}

export function exportAudit(format: 'jsonl' | 'csv', path: string, query: AuditQuery): Promise<number> {
  return invoke<number>('audit_export', { format, path, query });
}

export function setRetentionDays(days: number): Promise<number> {
  return invoke<number>('audit_set_retention_days', { days });
}

export function auditStats(): Promise<AuditStats> {
  return invoke<AuditStats>('audit_stats');
}

/** Tauri command errors arrive as plain strings. */
export function errorText(err: unknown): string {
  if (typeof err === 'string') return err;
  if (err instanceof Error) return err.message;
  return String(err);
}
