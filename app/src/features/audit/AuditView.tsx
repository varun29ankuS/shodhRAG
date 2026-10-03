import React, { useCallback, useEffect, useId, useRef, useState } from 'react';
import { ask, save } from '@tauri-apps/plugin-dialog';
import { Download, RefreshCw, ShieldCheck } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { auditStats, errorText, exportAudit, queryAudit, setRetentionDays, verifyAudit } from './api';
import { AuditTable } from './AuditTable';
import { dayBoundary, localDate } from './summary';
import { UsageCards } from './UsageCards';
import { EVENT_TYPES, MAX_RETENTION_DAYS, MIN_RETENTION_DAYS } from './types';
import type { AuditEventType, AuditQuery, AuditRow, AuditStats, VerifyReport } from './types';

const PAGE_SIZE = 200;
const SEARCH_DEBOUNCE_MS = 300;

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background';

const BUTTON = cn(
  'h-8 px-3 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border bg-shodh-surface text-[12.5px] text-shodh-text',
  'hover:bg-shodh-raised disabled:opacity-50 disabled:cursor-not-allowed transition-colors duration-micro',
  FOCUS_RING,
);

const INPUT = cn(
  'h-8 px-2.5 rounded-lg border border-shodh-border bg-shodh-surface text-[12.5px] text-shodh-text placeholder:text-shodh-text-faint',
  FOCUS_RING,
);

interface Filters {
  types: AuditEventType[];
  fromDate: string;
  toDate: string;
  conversationId: string;
  text: string;
}

const NO_FILTERS: Filters = { types: [], fromDate: '', toDate: '', conversationId: '', text: '' };

/** The query for `filters`, with `snapshot` capping `to` so paging is stable. */
function toQuery(filters: Filters, snapshot: string | null): AuditQuery {
  const query: AuditQuery = {};
  if (filters.types.length > 0) query.types = filters.types;
  const from = dayBoundary(filters.fromDate, 'start');
  if (from) query.from = from;
  const to = dayBoundary(filters.toDate, 'end');
  if (to && snapshot) query.to = to < snapshot ? to : snapshot;
  else if (to) query.to = to;
  else if (snapshot) query.to = snapshot;
  if (filters.conversationId.trim()) query.conversationId = filters.conversationId.trim();
  if (filters.text.trim()) query.text = filters.text.trim();
  return query;
}

function VerifyResult({ report, error }: { report: VerifyReport | null; error: string | null }) {
  if (error) return <span className="text-shodh-error">{error}</span>;
  if (!report) return null;
  if (report.ok) {
    return (
      <span className="text-shodh-success">
        ✓ {report.checked.toLocaleString()} {report.checked === 1 ? 'event' : 'events'} verified
      </span>
    );
  }
  return (
    <span className="text-shodh-error">
      ✗ Chain broken at event #{report.firstBadId ?? '?'}
      {report.reason ? ` — ${report.reason}` : ''}
    </span>
  );
}

/** Settings → Usage & Audit. */
export default function AuditView() {
  const ids = useId();
  const [stats, setStats] = useState<AuditStats | null>(null);
  const [unavailable, setUnavailable] = useState<string | null>(null);
  const [filters, setFilters] = useState<Filters>(NO_FILTERS);
  const [searchText, setSearchText] = useState('');
  const [rows, setRows] = useState<AuditRow[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);
  const [verifying, setVerifying] = useState(false);
  const [verifyReport, setVerifyReport] = useState<VerifyReport | null>(null);
  const [verifyError, setVerifyError] = useState<string | null>(null);
  const [exporting, setExporting] = useState(false);
  const [retentionInput, setRetentionInput] = useState('');
  const [savingRetention, setSavingRetention] = useState(false);
  const [reloadToken, setReloadToken] = useState(0);

  // Paging state that must not trigger renders: the snapshot time caps
  // `to` so new events do not shift offsets while paging.
  const snapshotRef = useRef<string | null>(null);
  const generationRef = useRef(0);
  const loadingRef = useRef(false);

  const loadStats = useCallback(async () => {
    try {
      const next = await auditStats();
      setStats(next);
      setUnavailable(null);
      setRetentionInput(current => (current === '' ? String(next.retentionDays) : current));
    } catch (err) {
      setUnavailable(errorText(err));
    }
  }, []);

  useEffect(() => {
    void loadStats();
  }, [loadStats]);

  // Debounce the free-text search into the filters.
  useEffect(() => {
    const timer = window.setTimeout(() => {
      setFilters(f => (f.text === searchText ? f : { ...f, text: searchText }));
    }, SEARCH_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [searchText]);

  // First page whenever the filters change (or on refresh).
  useEffect(() => {
    const generation = ++generationRef.current;
    snapshotRef.current = new Date().toISOString();
    loadingRef.current = true;
    setLoading(true);
    queryAudit({ ...toQuery(filters, snapshotRef.current), limit: PAGE_SIZE, offset: 0 })
      .then(page => {
        if (generation !== generationRef.current) return;
        setRows(page.rows);
        setTotal(page.total);
      })
      .catch(err => {
        if (generation !== generationRef.current) return;
        setRows([]);
        setTotal(0);
        setUnavailable(errorText(err));
      })
      .finally(() => {
        if (generation !== generationRef.current) return;
        loadingRef.current = false;
        setLoading(false);
      });
  }, [filters, reloadToken]);

  const loadMore = useCallback(() => {
    if (loadingRef.current) return;
    const generation = generationRef.current;
    loadingRef.current = true;
    setLoading(true);
    queryAudit({ ...toQuery(filters, snapshotRef.current), limit: PAGE_SIZE, offset: rows.length })
      .then(page => {
        if (generation !== generationRef.current) return;
        setRows(prev => {
          const seen = new Set(prev.map(r => r.id));
          return prev.concat(page.rows.filter(r => !seen.has(r.id)));
        });
        setTotal(page.total);
      })
      .catch(err => {
        if (generation !== generationRef.current) return;
        notify.error('Could not load more audit events', { description: errorText(err) });
      })
      .finally(() => {
        if (generation !== generationRef.current) return;
        loadingRef.current = false;
        setLoading(false);
      });
  }, [filters, rows.length]);

  const refresh = () => {
    setReloadToken(t => t + 1);
    void loadStats();
  };

  const toggleType = (type: AuditEventType) => {
    setFilters(f => ({
      ...f,
      types: f.types.includes(type) ? f.types.filter(t => t !== type) : [...f.types, type],
    }));
  };

  const clearFilters = () => {
    setSearchText('');
    setFilters(NO_FILTERS);
  };

  const runVerify = async () => {
    setVerifying(true);
    setVerifyError(null);
    setVerifyReport(null);
    try {
      setVerifyReport(await verifyAudit());
    } catch (err) {
      setVerifyError(`Verification failed: ${errorText(err)}`);
    } finally {
      setVerifying(false);
    }
  };

  const runExport = async (format: 'jsonl' | 'csv') => {
    setExporting(true);
    try {
      const path = await save({
        defaultPath: `shodh-audit-${localDate(new Date())}.${format}`,
        filters: [
          format === 'jsonl'
            ? { name: 'JSON Lines', extensions: ['jsonl'] }
            : { name: 'CSV', extensions: ['csv'] },
        ],
      });
      if (!path) return;
      const written = await exportAudit(format, path, toQuery(filters, null));
      notify.success(`Exported ${written.toLocaleString()} ${written === 1 ? 'event' : 'events'}`, {
        description: path,
      });
    } catch (err) {
      notify.error('Export failed', { description: errorText(err) });
    } finally {
      setExporting(false);
    }
  };

  const retentionDays = Number(retentionInput);
  const retentionValid =
    Number.isInteger(retentionDays) && retentionDays >= MIN_RETENTION_DAYS && retentionDays <= MAX_RETENTION_DAYS;
  const retentionChanged = stats !== null && retentionValid && retentionDays !== stats.retentionDays;

  const saveRetention = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!retentionValid) return;
    if (stats && retentionDays < stats.retentionDays) {
      const confirmed = await ask(
        `Events older than ${retentionDays} days will be permanently deleted from the audit log. A checkpoint records the deletion.`,
        { title: 'Shorten audit retention?', kind: 'warning', okLabel: 'Delete older events', cancelLabel: 'Cancel' },
      );
      if (!confirmed) return;
    }
    setSavingRetention(true);
    try {
      const deleted = await setRetentionDays(retentionDays);
      notify.success(`Audit events are kept for ${retentionDays} days`, {
        description:
          deleted > 0
            ? `${deleted.toLocaleString()} older ${deleted === 1 ? 'event was' : 'events were'} removed; a checkpoint keeps the chain verifiable.`
            : undefined,
      });
      refresh();
    } catch (err) {
      notify.error('Could not change retention', { description: errorText(err) });
    } finally {
      setSavingRetention(false);
    }
  };

  const hasFilters =
    filters.types.length > 0 || filters.fromDate !== '' || filters.toDate !== '' || filters.conversationId !== '' || searchText !== '';

  return (
    <div className="flex flex-col gap-6">
      {unavailable && (
        <p role="alert" className="m-0 rounded-lg border border-shodh-error/40 bg-shodh-surface px-3 py-2 text-[13px] text-shodh-error">
          {unavailable}
        </p>
      )}

      <UsageCards stats={stats} />

      <section aria-labelledby={`${ids}-integrity`} className="flex flex-col gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <h3 id={`${ids}-integrity`} className="m-0 mr-auto text-[14px] font-semibold">
            Event log
          </h3>
          <button type="button" className={BUTTON} onClick={runVerify} disabled={verifying}>
            <ShieldCheck aria-hidden="true" className="w-3.5 h-3.5" />
            {verifying ? 'Verifying…' : 'Verify integrity'}
          </button>
          <button type="button" className={BUTTON} onClick={() => void runExport('jsonl')} disabled={exporting}>
            <Download aria-hidden="true" className="w-3.5 h-3.5" />
            Export JSONL
          </button>
          <button type="button" className={BUTTON} onClick={() => void runExport('csv')} disabled={exporting}>
            <Download aria-hidden="true" className="w-3.5 h-3.5" />
            Export CSV
          </button>
          <button type="button" className={BUTTON} onClick={refresh} disabled={loading} aria-label="Refresh events">
            <RefreshCw aria-hidden="true" className={cn('w-3.5 h-3.5', loading && 'agent-spin')} />
          </button>
        </div>
        <p aria-live="polite" className="m-0 min-h-[18px] text-[12.5px]">
          <VerifyResult report={verifyReport} error={verifyError} />
        </p>
        <p className="m-0 text-[12px] text-shodh-text-muted">
          Every question, tool call, approval, retrieval, answer and settings or source change is recorded in{' '}
          <span className="font-mono">shodh.db</span> as a hash chain; editing, deleting or reordering an event breaks it.{' '}
          {stats
            ? stats.encrypted
              ? 'The audit database is encrypted with a key held in the OS credential store.'
              : 'The audit database is not encrypted in this build; protect it with disk encryption (BitLocker or FileVault).'
            : null}
        </p>
      </section>

      <div role="group" aria-label="Filter events" className="flex flex-col gap-2.5">
        <div className="flex flex-wrap gap-1.5" role="group" aria-label="Event types">
          {EVENT_TYPES.map(t => {
            const active = filters.types.includes(t.id);
            const count = stats?.byType[t.id];
            return (
              <button
                key={t.id}
                type="button"
                aria-pressed={active}
                onClick={() => toggleType(t.id)}
                className={cn(
                  'h-7 px-2.5 rounded-full border text-[12px] transition-colors duration-micro',
                  active
                    ? 'border-shodh-accent bg-shodh-accent-soft text-shodh-accent-text'
                    : 'border-shodh-border-subtle text-shodh-text-secondary hover:bg-shodh-raised',
                  FOCUS_RING,
                )}
              >
                {t.label}
                {count !== undefined && <span className="ml-1 tabular-nums text-shodh-text-faint">{count}</span>}
              </button>
            );
          })}
        </div>
        <div className="flex flex-wrap items-end gap-2">
          <label className="flex flex-col gap-1 text-[11.5px] text-shodh-text-muted">
            From
            <input
              type="date"
              className={INPUT}
              value={filters.fromDate}
              max={filters.toDate || undefined}
              onChange={e => setFilters(f => ({ ...f, fromDate: e.target.value }))}
            />
          </label>
          <label className="flex flex-col gap-1 text-[11.5px] text-shodh-text-muted">
            To
            <input
              type="date"
              className={INPUT}
              value={filters.toDate}
              min={filters.fromDate || undefined}
              onChange={e => setFilters(f => ({ ...f, toDate: e.target.value }))}
            />
          </label>
          <label className="flex flex-col gap-1 text-[11.5px] text-shodh-text-muted">
            Conversation
            <input
              type="text"
              className={cn(INPUT, 'w-[180px] font-mono')}
              placeholder="Conversation id"
              value={filters.conversationId}
              onChange={e => setFilters(f => ({ ...f, conversationId: e.target.value }))}
            />
          </label>
          <label className="flex flex-col gap-1 text-[11.5px] text-shodh-text-muted flex-1 min-w-[180px]">
            Search
            <input
              type="search"
              className={INPUT}
              placeholder="Text in the event, e.g. a file name or provider"
              value={searchText}
              onChange={e => setSearchText(e.target.value)}
            />
          </label>
          {hasFilters && (
            <button type="button" className={BUTTON} onClick={clearFilters}>
              Clear filters
            </button>
          )}
        </div>
      </div>

      <AuditTable
        rows={rows}
        total={total}
        loading={loading}
        onLoadMore={loadMore}
        onSelectConversation={conversationId => setFilters(f => ({ ...f, conversationId }))}
      />

      <form onSubmit={saveRetention} className="flex flex-wrap items-end gap-2" aria-labelledby={`${ids}-retention`}>
        <div className="flex flex-col gap-1 mr-auto">
          <h3 id={`${ids}-retention`} className="m-0 text-[14px] font-semibold">
            Retention
          </h3>
          <p id={`${ids}-retention-help`} className="m-0 text-[12px] text-shodh-text-muted">
            Events older than this are deleted at startup and when you save. A checkpoint records each trim.
            {stats?.oldest ? ` Oldest event: ${new Date(stats.oldest).toLocaleDateString()}.` : ''}
          </p>
        </div>
        <label className="flex flex-col gap-1 text-[11.5px] text-shodh-text-muted">
          Keep events for (days)
          <input
            type="number"
            inputMode="numeric"
            min={MIN_RETENTION_DAYS}
            max={MAX_RETENTION_DAYS}
            step={1}
            required
            aria-describedby={`${ids}-retention-help`}
            aria-invalid={retentionInput !== '' && !retentionValid}
            className={cn(INPUT, 'w-[120px] tabular-nums')}
            value={retentionInput}
            onChange={e => setRetentionInput(e.target.value)}
          />
        </label>
        <button type="submit" className={BUTTON} disabled={!retentionChanged || savingRetention}>
          {savingRetention ? 'Saving…' : 'Save'}
        </button>
      </form>
    </div>
  );
}
