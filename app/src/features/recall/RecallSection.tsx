import React, { useCallback, useEffect, useId, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { AlertTriangle, Bot, ChevronDown, ChevronRight, History, Lock, Play, RefreshCw, Search, ShieldCheck, Trash2, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { agentItemKey } from '../../lib/agentHighlight';
import { recallApi, RECALL_EVENT } from './api';
import type { RecallAssistantAccess, RecallScopeAnswer, RecallSearchAnswer, RecallSettings, RecallStatus } from './api';
import {
  assistantLine,
  dayOf,
  parsePatterns,
  staleLine,
  recallStage,
  recallStatusLine,
  toolLabel,
  waitingLine,
  withProject,
  withTool,
} from './recallModel';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';
const PRIMARY_BUTTON = cn(
  'h-9 px-3.5 inline-flex items-center gap-2 rounded-lg bg-shodh-accent text-shodh-on-accent text-small font-semibold hover:bg-shodh-accent-hover transition-colors duration-micro disabled:opacity-60',
  FOCUS_RING,
);
const QUIET_BUTTON = cn(
  'h-8 px-2.5 inline-flex items-center gap-1.5 rounded-lg text-small text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro disabled:opacity-50',
  FOCUS_RING,
);
const CHECKBOX = cn('w-3.5 h-3.5 accent-[var(--c-accent)] rounded', FOCUS_RING);

/** How soon the status is read again after a progress event. */
const EVENT_REFRESH_MS = 800;
/** ... and while a sync runs. */
const BUSY_REFRESH_MS = 4000;

function errorText(err: unknown): string {
  if (typeof err === 'string') return err;
  if (err instanceof Error) return err.message;
  return String(err);
}

/** Recall's status, kept current from its events. */
function useRecallStatus() {
  const [status, setStatus] = useState<RecallStatus | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const mounted = useRef(true);
  const refresh = useCallback(() => {
    recallApi
      .status()
      .then(s => {
        if (mounted.current) {
          setStatus(s);
          setLoadError(null);
        }
      })
      .catch(e => mounted.current && setLoadError(errorText(e)));
  }, []);
  useEffect(() => {
    mounted.current = true;
    refresh();
    let timer: number | null = null;
    const unlisten = listen(RECALL_EVENT, () => {
      if (timer !== null) return;
      timer = window.setTimeout(() => {
        timer = null;
        refresh();
      }, EVENT_REFRESH_MS);
    });
    return () => {
      mounted.current = false;
      if (timer !== null) window.clearTimeout(timer);
      unlisten.then(fn => fn()).catch(() => undefined);
    };
  }, [refresh]);
  const busy = status?.running != null;
  useEffect(() => {
    if (!busy) return;
    const timer = window.setInterval(refresh, BUSY_REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [busy, refresh]);
  return { status, setStatus, loadError, refresh };
}

interface RecallSectionProps {
  /** Open a receipt in the session viewer. */
  onOpenReceipt: (receipt: string) => void;
}

/**
 * The Library's "AI coding sessions" source: the user's Claude Code and Codex sessions,
 * read where they are and searched on this computer. Off until the user turns it on,
 * then only the tools and projects they choose.
 */
export function RecallSection({ onOpenReceipt }: RecallSectionProps) {
  const { status, setStatus, loadError, refresh } = useRecallStatus();
  const [saving, setSaving] = useState(false);
  const headingId = useId();

  const save = useCallback(
    async (settings: RecallSettings, done?: string) => {
      setSaving(true);
      try {
        setStatus(await recallApi.saveSettings(settings));
        if (done) notify.success(done);
      } catch (e) {
        notify.error('Recall settings were not saved', { description: errorText(e) });
      } finally {
        setSaving(false);
      }
    },
    [setStatus],
  );

  return (
    <section aria-labelledby={headingId} data-agent-item={agentItemKey('library-section', 'recall')} className="flex flex-col gap-3">
      <div className="flex flex-col gap-0.5">
        <h2 id={headingId} className="text-caption font-semibold uppercase tracking-[0.08em] text-shodh-text-faint">
          AI coding sessions
        </h2>
        <p className="text-small text-shodh-text-muted">
          What you did in Claude Code and Codex, searchable here and by the assistant. Shodh reads the transcripts where
          they are, on this computer.
        </p>
      </div>
      <div className="rounded-xl border border-shodh-border bg-shodh-surface p-4 flex flex-col gap-4">
        {!status ? (
          <p className="m-0 text-small text-shodh-text-muted">{loadError ? `Recall could not be read: ${loadError}` : 'Reading Recall…'}</p>
        ) : (
          <RecallCard status={status} saving={saving} onSave={save} onRefresh={refresh} onOpenReceipt={onOpenReceipt} />
        )}
      </div>
    </section>
  );
}

function PrivacyNote({ status }: { status: RecallStatus }) {
  return (
    <ul className="m-0 p-0 list-none flex flex-col gap-1.5 text-small text-shodh-text-secondary">
      <li className="flex items-start gap-2">
        <Lock className="w-3.5 h-3.5 mt-0.5 shrink-0 text-shodh-text-muted" aria-hidden="true" />
        <span>Transcripts stay where the tools keep them. Recall&apos;s index of them is kept on this computer.</span>
      </li>
      <li className="flex items-start gap-2">
        <ShieldCheck className="w-3.5 h-3.5 mt-0.5 shrink-0 text-shodh-text-muted" aria-hidden="true" />
        <span>
          Keys, tokens, passwords, private keys{status.settings.keepEmails ? '' : ', email addresses'} and your hidden words are removed
          before anything is indexed or shown. Long random ids, such as request ids and hashes, are hidden too.
        </span>
      </li>
      <li className="flex items-start gap-2">
        <Bot className="w-3.5 h-3.5 mt-0.5 shrink-0 text-shodh-text-muted" aria-hidden="true" />
        <span>{assistantLine(status.settings.assistant, status.assistantModel)}</span>
      </li>
      <li className="flex items-start gap-2">
        <History className="w-3.5 h-3.5 mt-0.5 shrink-0 text-shodh-text-muted" aria-hidden="true" />
        <span>Only the tools and projects you choose are read. Slack and Telegram answers never use them.</span>
      </li>
    </ul>
  );
}

const ACCESS_CHOICES: { value: RecallAssistantAccess; label: string }[] = [
  { value: 'off', label: 'No' },
  { value: 'local_models', label: 'Only with a model on this computer' },
  { value: 'any_model', label: 'Yes, with the model I choose' },
];

/** Whether the assistant may read the sessions: off until the user says so. */
function AssistantChoice({ status, saving, onSave }: { status: RecallStatus; saving: boolean; onSave: CardProps['onSave'] }) {
  const settings = status.settings;
  const name = useId();
  return (
    <fieldset className="m-0 p-0 border-0 flex flex-col gap-1.5">
      <legend className="text-small font-semibold text-shodh-text mb-1">Let the assistant read your coding sessions</legend>
      {ACCESS_CHOICES.map(choice => (
        <label key={choice.value} className="flex items-center gap-2 text-small text-shodh-text cursor-pointer">
          <input
            type="radio"
            name={name}
            className={CHECKBOX}
            checked={settings.assistant === choice.value}
            disabled={saving}
            onChange={() => void onSave({ ...settings, assistant: choice.value })}
          />
          {choice.label}
        </label>
      ))}
      <p className="m-0 text-caption text-shodh-text-muted">{assistantLine(settings.assistant, status.assistantModel)}</p>
    </fieldset>
  );
}

interface CardProps {
  status: RecallStatus;
  saving: boolean;
  onSave: (settings: RecallSettings, done?: string) => Promise<void>;
  onRefresh: () => void;
  onOpenReceipt: (receipt: string) => void;
}

function RecallCard({ status, saving, onSave, onRefresh, onOpenReceipt }: CardProps) {
  const stage = recallStage(status);
  const settings = status.settings;
  const [showChoices, setShowChoices] = useState(false);
  const [confirmOff, setConfirmOff] = useState(false);
  const choicesId = useId();

  if (stage === 'off') {
    return (
      <div className="flex flex-col gap-3">
        <p className="m-0 text-body text-shodh-text">
          Index your Claude Code and Codex sessions to search what you did. Nothing is read until you choose the tools and the
          projects.
        </p>
        <PrivacyNote status={status} />
        <div>
          <button type="button" className={PRIMARY_BUTTON} disabled={saving} onClick={() => void onSave({ ...settings, enabled: true })}>
            <Play className="w-4 h-4" aria-hidden="true" />
            Set up
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-4">
      {status.error && (
        <p className="m-0 text-small text-shodh-warning flex items-start gap-1.5" role="status">
          <AlertTriangle className="w-3.5 h-3.5 mt-0.5 shrink-0" aria-hidden="true" />
          <span className="break-words">{status.error}</span>
        </p>
      )}
      {stage === 'tools' && (
        <>
          <ToolChoice status={status} saving={saving} onSave={onSave} />
          <div>
            <button type="button" className={QUIET_BUTTON} disabled={saving} onClick={() => void onSave({ ...settings, enabled: false })}>
              <X className="w-3.5 h-3.5" aria-hidden="true" />
              Not now
            </button>
          </div>
        </>
      )}
      {stage === 'projects' && <ProjectChoice status={status} saving={saving} onSave={onSave} firstTime />}
      {stage === 'ready' && (
        <>
          <div className="flex flex-wrap items-center justify-between gap-2">
            <p className="m-0 text-small text-shodh-text-secondary tabular-nums" role="status">
              {recallStatusLine(status, new Date())}
            </p>
            <button type="button" className={QUIET_BUTTON} onClick={() => void recallApi.syncNow().then(onRefresh).catch(e => notify.error('Recall could not look now', { description: errorText(e) }))}>
              <RefreshCw className="w-3.5 h-3.5" aria-hidden="true" />
              Look for new sessions
            </button>
          </div>
          {waitingLine(status) && <p className="m-0 text-caption text-shodh-text-muted">{waitingLine(status)}</p>}
          {staleLine(status) && <p className="m-0 text-caption text-shodh-text-muted">{staleLine(status)}</p>}
          <RecallSearch onOpenReceipt={onOpenReceipt} />
        </>
      )}
      {stage !== 'tools' && (
        <div className="flex flex-col gap-2">
          <button
            type="button"
            onClick={() => setShowChoices(v => !v)}
            aria-expanded={showChoices}
            aria-controls={choicesId}
            className={cn('self-start inline-flex items-center gap-1 text-small text-shodh-text-secondary hover:text-shodh-text rounded', FOCUS_RING)}
          >
            {showChoices ? <ChevronDown className="w-3.5 h-3.5" aria-hidden="true" /> : <ChevronRight className="w-3.5 h-3.5" aria-hidden="true" />}
            Tools, projects and privacy
          </button>
          {showChoices && (
            <div id={choicesId} className="flex flex-col gap-4 rounded-lg bg-shodh-raised p-3">
              <ToolChoice status={status} saving={saving} onSave={onSave} />
              <ProjectChoice status={status} saving={saving} onSave={onSave} />
              <AssistantChoice status={status} saving={saving} onSave={onSave} />
              <PatternChoice settings={settings} saving={saving} onSave={onSave} />
              <label className="flex items-start gap-2 text-small text-shodh-text cursor-pointer">
                <input
                  type="checkbox"
                  className={cn(CHECKBOX, 'mt-0.5')}
                  checked={settings.keepEmails}
                  disabled={saving}
                  onChange={e => void onSave({ ...settings, keepEmails: e.target.checked })}
                />
                <span className="flex flex-col">
                  <span>Keep email addresses</span>
                  <span className="text-caption text-shodh-text-muted">Off: they are hidden like keys. Changing it indexes every session again.</span>
                </span>
              </label>
              <PrivacyNote status={status} />
              {confirmOff ? (
                <div className="flex flex-wrap items-center gap-2" role="group" aria-label="Turn Recall off">
                  <span className="text-small text-shodh-text">Delete Recall's index? Your transcripts are not touched.</span>
                  <button
                    type="button"
                    className={cn(QUIET_BUTTON, 'text-shodh-error hover:text-shodh-error border border-shodh-border')}
                    disabled={saving}
                    onClick={() => {
                      setConfirmOff(false);
                      void onSave({ ...settings, enabled: false }, 'Recall is off and its index is being deleted');
                    }}
                  >
                    <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
                    Delete the index
                  </button>
                  <button type="button" className={QUIET_BUTTON} onClick={() => setConfirmOff(false)}>
                    <X className="w-3.5 h-3.5" aria-hidden="true" />
                    Keep it
                  </button>
                </div>
              ) : (
                <div>
                  <button
                    type="button"
                    className={cn(QUIET_BUTTON, 'text-shodh-error hover:text-shodh-error')}
                    disabled={saving}
                    onClick={() => setConfirmOff(true)}
                  >
                    <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
                    Turn off and delete the index
                  </button>
                </div>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function ToolChoice({ status, saving, onSave }: { status: RecallStatus; saving: boolean; onSave: CardProps['onSave'] }) {
  const settings = status.settings;
  return (
    <fieldset className="m-0 p-0 border-0 flex flex-col gap-2">
      <legend className="text-small font-semibold text-shodh-text mb-1">Which tools to read</legend>
      {status.tools.map(tool => (
        <label key={tool.id} className="flex items-start gap-2 text-small text-shodh-text cursor-pointer">
          <input
            type="checkbox"
            className={cn(CHECKBOX, 'mt-0.5')}
            checked={settings.tools.includes(tool.id)}
            disabled={saving}
            onChange={e => void onSave(withTool(settings, tool.id, e.target.checked))}
          />
          <span className="flex flex-col">
            <span>{tool.label}</span>
            <span className="text-caption text-shodh-text-muted break-all">
              {tool.files > 0 ? `${tool.files.toLocaleString()} session ${tool.files === 1 ? 'file' : 'files'} in ${tool.folder ?? 'its folder'}` : `No sessions found${tool.folder ? ` in ${tool.folder}` : ''}`}
            </span>
          </span>
        </label>
      ))}
    </fieldset>
  );
}

function ProjectChoice({
  status,
  saving,
  onSave,
  firstTime = false,
}: {
  status: RecallStatus;
  saving: boolean;
  onSave: CardProps['onSave'];
  firstTime?: boolean;
}) {
  const settings = status.settings;
  const projects = status.projects;
  return (
    <fieldset className="m-0 p-0 border-0 flex flex-col gap-2">
      <legend className="text-small font-semibold text-shodh-text mb-1">Which projects to index</legend>
      {projects.length === 0 ? (
        <p className="m-0 text-small text-shodh-text-muted">
          {status.running ? 'Looking for projects in the transcripts…' : 'No projects found yet for the chosen tools.'}
        </p>
      ) : (
        <>
          <p className="m-0 text-caption text-shodh-text-muted">
            {firstTime ? 'Nothing of a project is indexed until you include it. ' : ''}A session that also worked in a project you
            excluded is not indexed. A worktree or a copy in another folder is a project of its own.
          </p>
          <ul className="m-0 p-0 list-none flex flex-col gap-1.5 max-h-64 overflow-y-auto scrollbar-thin">
            {projects.map(project => (
              <li key={project.key}>
                <label className="flex items-start gap-2 text-small text-shodh-text cursor-pointer">
                  <input
                    type="checkbox"
                    className={cn(CHECKBOX, 'mt-0.5')}
                    checked={project.included === true}
                    disabled={saving}
                    onChange={e => void onSave(withProject(settings, project.key, e.target.checked))}
                  />
                  <span className="min-w-0 flex flex-col">
                    <span className="truncate" title={project.dir}>{project.name}</span>
                    <span className="text-caption text-shodh-text-muted break-all">{project.dir}</span>
                    <span className="text-caption text-shodh-text-muted">
                      {`${project.sessions.toLocaleString()} ${project.sessions === 1 ? 'session' : 'sessions'}`}
                      {project.included ? ` · ${project.indexedSessions.toLocaleString()} indexed` : project.included === false ? ' · excluded' : ' · not included yet'}
                      {project.failed > 0 ? ` · ${project.failed.toLocaleString()} could not be read` : ''}
                      {project.lastActive ? ` · last ${dayOf(project.lastActive)}` : ''}
                    </span>
                  </span>
                </label>
              </li>
            ))}
          </ul>
        </>
      )}
    </fieldset>
  );
}

function PatternChoice({ settings, saving, onSave }: { settings: RecallSettings; saving: boolean; onSave: CardProps['onSave'] }) {
  const [text, setText] = useState(settings.patterns.join('\n'));
  const fieldId = useId();
  useEffect(() => setText(settings.patterns.join('\n')), [settings.patterns]);
  const changed = parsePatterns(text).join('\n') !== settings.patterns.join('\n');
  return (
    <div className="flex flex-col gap-1.5">
      <label htmlFor={fieldId} className="text-small font-semibold text-shodh-text">
        Your own hidden words
      </label>
      <p className="m-0 text-caption text-shodh-text-muted">
        One a line: a word, a name or a code to hide like a key, in any case (a client&apos;s name, an internal host). To hide a
        family of codes, write a pattern between slashes, like /ACME-[0-9]{'{6}'}/. Changing them indexes every session again.
      </p>
      <textarea
        id={fieldId}
        value={text}
        onChange={e => setText(e.target.value)}
        rows={3}
        spellCheck={false}
        className={cn('w-full rounded-lg border border-shodh-border bg-shodh-surface p-2 font-mono text-small text-shodh-text', FOCUS_RING)}
        placeholder={'Globex\ninternal.example.com\n/ACME-[0-9]{6}/'}
      />
      <div>
        <button
          type="button"
          className={QUIET_BUTTON}
          disabled={saving || !changed}
          onClick={() => void onSave({ ...settings, patterns: parsePatterns(text) }, 'Hidden words saved')}
        >
          <ShieldCheck className="w-3.5 h-3.5" aria-hidden="true" />
          Save hidden words
        </button>
      </div>
    </div>
  );
}

function RecallSearch({ onOpenReceipt }: { onOpenReceipt: (receipt: string) => void }) {
  const [query, setQuery] = useState('');
  const [answer, setAnswer] = useState<RecallSearchAnswer | null>(null);
  const [recent, setRecent] = useState<RecallScopeAnswer | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const fieldId = useId();

  useEffect(() => {
    let live = true;
    recallApi
      .scope({ period: 'last 7 days' })
      .then(r => live && setRecent(r))
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, []);

  const run = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!query.trim()) return;
    setBusy(true);
    setError(null);
    try {
      setAnswer(await recallApi.search({ query: query.trim(), limit: 10 }));
    } catch (err) {
      setError(errorText(err));
      setAnswer(null);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-col gap-3">
      <form onSubmit={run} className="flex items-center gap-2" role="search">
        <label htmlFor={fieldId} className="sr-only">
          Search your coding sessions
        </label>
        <input
          id={fieldId}
          value={query}
          onChange={e => setQuery(e.target.value)}
          placeholder="A bug, a decision, a PR number, a file…"
          className={cn('h-9 flex-1 min-w-0 rounded-lg border border-shodh-border bg-shodh-surface px-3 text-small text-shodh-text', FOCUS_RING)}
        />
        <button type="submit" className={PRIMARY_BUTTON} disabled={busy || !query.trim()}>
          <Search className="w-4 h-4" aria-hidden="true" />
          Search
        </button>
      </form>
      {error && <p className="m-0 text-small text-shodh-error">{error}</p>}
      {answer && (
        <div className="flex flex-col gap-2" aria-live="polite">
          <p className="m-0 text-caption text-shodh-text-muted tabular-nums">
            {answer.sessions.length === 0
              ? `No session matched (${answer.searched.toLocaleString()} searched).`
              : `${answer.sessions.length} of ${answer.searched.toLocaleString()} sessions, in ${answer.tookMs.toLocaleString()} ms`}
          </p>
          <ul className="m-0 p-0 list-none flex flex-col gap-2">
            {answer.sessions.map(s => (
              <li key={`${s.tool}:${s.sessionId}`} className="rounded-lg border border-shodh-border p-3 flex flex-col gap-1.5">
                <p className="m-0 text-small font-semibold text-shodh-text">{s.title}</p>
                <p className="m-0 text-caption text-shodh-text-muted">
                  {[toolLabel(s.tool), s.project, dayOf(s.started), s.branch].filter(Boolean).join(' · ')}
                  {s.exact.length > 0 ? ` · holds ${s.exact.join(', ')}` : ''}
                </p>
                <ul className="m-0 p-0 list-none flex flex-col gap-1">
                  {s.receipts.map(r => (
                    <li key={r.id}>
                      <button
                        type="button"
                        onClick={() => onOpenReceipt(r.id)}
                        className={cn('w-full text-left rounded-md px-2 py-1 hover:bg-shodh-raised flex flex-col', FOCUS_RING)}
                      >
                        <span className="text-caption font-medium text-shodh-text-secondary">{r.heading}</span>
                        {r.snippet && <span className="text-caption text-shodh-text-muted line-clamp-2 break-words">{r.snippet}</span>}
                      </button>
                    </li>
                  ))}
                </ul>
              </li>
            ))}
          </ul>
        </div>
      )}
      {!answer && recent && recent.sessions.length > 0 && (
        <div className="flex flex-col gap-1.5">
          <p className="m-0 text-caption font-semibold text-shodh-text-secondary">The last 7 days</p>
          <ul className="m-0 p-0 list-none flex flex-col gap-1">
            {[...recent.sessions].reverse().slice(0, 12).map(s => (
              <li key={`${s.tool}:${s.sessionId}`}>
                <button
                  type="button"
                  disabled={!s.first}
                  onClick={() => s.first && onOpenReceipt(s.first)}
                  className={cn('w-full text-left rounded-md px-2 py-1 hover:bg-shodh-raised flex flex-col disabled:opacity-60', FOCUS_RING)}
                >
                  <span className="text-small text-shodh-text truncate">{s.title}</span>
                  <span className="text-caption text-shodh-text-muted">
                    {[toolLabel(s.tool), s.project, dayOf(s.started), `${s.turns.toLocaleString()} turns`].filter(Boolean).join(' · ')}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

