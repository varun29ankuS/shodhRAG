import React, { useCallback, useEffect, useId, useRef, useState } from 'react';
import { open as pickFolder } from '@tauri-apps/plugin-dialog';
import { openUrl } from '@tauri-apps/plugin-opener';
import { ChevronDown, ChevronRight, FileJson, Plus, RefreshCw, Trash2 } from 'lucide-react';
import { cn } from '../lib/utils';
import { notify } from '../lib/notify';
import { useWorkspaces } from '../features/workspaces/WorkspaceContext';
import { errorText, toolsApi } from '../features/tools/api';
import type { Approval, ServerView, StagedInstall, ToolMode, ToolsOverview } from '../features/tools/api';
import { formToConfig } from '../features/tools/format';
import type { ServerForm } from '../features/tools/format';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

const BUTTON = cn(
  'inline-flex items-center gap-1.5 h-8 px-3 rounded-lg text-[12.5px] font-medium border border-shodh-border text-shodh-text-secondary hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro disabled:opacity-50 disabled:cursor-not-allowed',
  FOCUS_RING,
);
const INPUT = cn(
  'w-full rounded-lg border border-shodh-border bg-shodh-ground px-2.5 py-1.5 text-[13px] text-shodh-text placeholder:text-shodh-text-faint',
  FOCUS_RING,
);
const SECTION_HEADING = 'm-0 text-[15px] font-semibold text-shodh-text';

type ModeChoice = 'both' | ToolMode;

function modeChoice(modes: ToolMode[]): ModeChoice {
  return modes.length === 1 ? modes[0] : 'both';
}

function StatusDot({ server }: { server: ServerView }) {
  const [color, text] = !server.enabled
    ? ['bg-shodh-text-faint', 'Off']
    : server.status === 'connected'
      ? ['bg-shodh-success', 'Connected']
      : server.status === 'error'
        ? ['bg-shodh-error', 'Not working']
        : ['bg-shodh-text-faint', 'Not started yet'];
  return (
    <span className="inline-flex items-center gap-1.5 text-[11.5px] text-shodh-text-muted shrink-0">
      <span className={cn('w-2 h-2 rounded-full', color)} aria-hidden="true" />
      {text}
    </span>
  );
}

function Toggle({ label, checked, onChange, disabled }: { label: string; checked: boolean; onChange: (on: boolean) => void; disabled?: boolean }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cn(
        'relative shrink-0 w-9 h-5 rounded-full transition-colors duration-micro disabled:opacity-50 disabled:cursor-not-allowed',
        checked ? 'bg-shodh-accent' : 'bg-shodh-raised-2',
        FOCUS_RING,
      )}
    >
      <span
        aria-hidden="true"
        className={cn('absolute top-0.5 w-4 h-4 rounded-full bg-white shadow transition-[left] duration-micro', checked ? 'left-[18px]' : 'left-0.5')}
      />
    </button>
  );
}

interface ServerCardProps {
  server: ServerView;
  workspaceId: string | null;
  onChanged: () => void;
  onTested: (server: ServerView) => void;
}

function ServerCard({ server, workspaceId, onChanged, onTested }: ServerCardProps) {
  const [expanded, setExpanded] = useState(false);
  const [testing, setTesting] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const toolsId = useId();

  const run = async (action: () => Promise<unknown>, failure: string) => {
    try {
      await action();
      onChanged();
    } catch (e) {
      notify.error(failure, { description: errorText(e) });
    }
  };

  const test = async () => {
    setTesting(true);
    try {
      onTested(await toolsApi.testServer(workspaceId, server.name));
    } catch (e) {
      notify.error(`${server.name} could not be tested`, { description: errorText(e) });
    } finally {
      setTesting(false);
    }
  };

  return (
    <li className="rounded-xl border border-shodh-border-subtle bg-shodh-surface-2 p-3.5 flex flex-col gap-2.5">
      <div className="flex items-center gap-3 min-w-0">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2 min-w-0">
            <h4 className="m-0 text-[13.5px] font-semibold text-shodh-text truncate">{server.name}</h4>
            <StatusDot server={server} />
          </div>
          <p className="m-0 mt-0.5 text-[11.5px] font-mono text-shodh-text-muted truncate" title={server.target}>
            {server.kind === 'http' ? server.target : `$ ${server.target}`}
            {server.needsFolder && ' (in the code folder)'}
          </p>
        </div>
        <label className="sr-only" htmlFor={`${toolsId}-mode`}>Modes for {server.name}</label>
        <select
          id={`${toolsId}-mode`}
          value={modeChoice(server.modes)}
          onChange={e => {
            const choice = e.target.value as ModeChoice;
            const modes: ToolMode[] = choice === 'both' ? ['research', 'code'] : [choice];
            void run(() => toolsApi.setServer(workspaceId, server.name, { modes }), 'The modes were not saved');
          }}
          className={cn('h-8 rounded-lg border border-shodh-border bg-shodh-ground px-2 text-[12.5px] text-shodh-text', FOCUS_RING)}
        >
          <option value="both">Research and Code</option>
          <option value="research">Research only</option>
          <option value="code">Code only</option>
        </select>
        <Toggle
          label={`Use ${server.name}`}
          checked={server.enabled}
          onChange={enabled => void run(() => toolsApi.setServer(workspaceId, server.name, { enabled }), 'The server was not changed')}
        />
      </div>
      {server.error && (
        <p role="status" className="m-0 text-[12px] text-shodh-error break-words">
          {server.error}
        </p>
      )}
      <div className="flex items-center gap-2 flex-wrap">
        <button
          type="button"
          onClick={() => setExpanded(x => !x)}
          aria-expanded={expanded}
          aria-controls={toolsId}
          disabled={server.tools.length === 0}
          className={cn('inline-flex items-center gap-1 text-[12.5px] text-shodh-text-secondary hover:text-shodh-text disabled:text-shodh-text-faint rounded', FOCUS_RING)}
        >
          {expanded ? <ChevronDown className="w-3.5 h-3.5" aria-hidden="true" /> : <ChevronRight className="w-3.5 h-3.5" aria-hidden="true" />}
          {server.tools.length === 0
            ? 'Tools appear once it connects'
            : `${server.tools.filter(t => t.enabled).length} of ${server.tools.length} tools on`}
        </button>
        <span className="ml-auto" />
        <button type="button" onClick={() => void test()} disabled={testing} className={BUTTON}>
          <RefreshCw className={cn('w-3.5 h-3.5', testing && 'animate-spin')} aria-hidden="true" />
          {testing ? 'Testing…' : 'Test connection'}
        </button>
        <button
          type="button"
          onClick={() => {
            if (!confirmRemove) {
              setConfirmRemove(true);
              return;
            }
            void run(() => toolsApi.removeServer(workspaceId, server.name), 'The server was not removed');
          }}
          onBlur={() => setConfirmRemove(false)}
          className={cn(BUTTON, confirmRemove && 'border-shodh-error text-shodh-error')}
        >
          <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
          {confirmRemove ? 'Confirm remove' : 'Remove'}
        </button>
      </div>
      {expanded && server.tools.length > 0 && (
        <ul id={toolsId} className="m-0 p-0 list-none flex flex-col divide-y divide-shodh-border-subtle">
          {server.tools.map(tool => (
            <li key={tool.name} className="flex items-start gap-3 py-2">
              <Toggle
                label={`Use ${tool.name}`}
                checked={tool.enabled}
                onChange={enabled =>
                  void run(() => toolsApi.setTool(workspaceId, server.name, tool.name, { enabled }), 'The tool was not changed')
                }
              />
              <div className="min-w-0 flex-1">
                <p className="m-0 text-[12.5px] font-medium text-shodh-text break-words">
                  {tool.title ?? tool.name}
                  {tool.readOnly && <span className="ml-1.5 text-[11px] font-normal text-shodh-text-faint">read-only</span>}
                </p>
                {tool.description && (
                  <p className="m-0 text-[12px] text-shodh-text-muted line-clamp-2" title={tool.description}>
                    {tool.description}
                  </p>
                )}
              </div>
              <label className="sr-only" htmlFor={`${toolsId}-${tool.name}`}>Approval for {tool.name}</label>
              <select
                id={`${toolsId}-${tool.name}`}
                value={tool.approval}
                onChange={e =>
                  void run(
                    () => toolsApi.setTool(workspaceId, server.name, tool.name, { approval: e.target.value as Approval }),
                    'The approval setting was not saved',
                  )
                }
                className={cn('h-7 rounded-md border border-shodh-border bg-shodh-ground px-1.5 text-[12px] text-shodh-text', FOCUS_RING)}
              >
                <option value="ask">Ask first</option>
                <option value="auto">Run automatically</option>
              </select>
            </li>
          ))}
        </ul>
      )}
    </li>
  );
}

function AddServer({ workspaceId, onAdded }: { workspaceId: string | null; onAdded: () => void }) {
  const [tab, setTab] = useState<'paste' | 'form'>('paste');
  const [pasted, setPasted] = useState('');
  const [form, setForm] = useState<ServerForm>({ name: '', kind: 'stdio', target: '', secrets: '' });
  const [problem, setProblem] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const ids = useId();

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    let text = pasted;
    if (tab === 'form') {
      const made = formToConfig(form);
      if ('error' in made) {
        setProblem(made.error);
        return;
      }
      text = made.config;
    }
    setSaving(true);
    try {
      const names = await toolsApi.addServers(workspaceId, text);
      notify.success(names.length === 1 ? `Added ${names[0]}` : `Added ${names.length} servers`);
      setPasted('');
      setForm({ name: '', kind: 'stdio', target: '', secrets: '' });
      setProblem(null);
      onAdded();
    } catch (e) {
      setProblem(errorText(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <form onSubmit={e => void submit(e)} className="rounded-xl border border-shodh-border bg-shodh-surface-2 p-3.5 flex flex-col gap-3">
      <div role="tablist" aria-label="How to add a server" className="flex gap-1">
        {(['paste', 'form'] as const).map(id => (
          <button
            key={id}
            type="button"
            role="tab"
            aria-selected={tab === id}
            onClick={() => {
              setTab(id);
              setProblem(null);
            }}
            className={cn(
              'h-7 px-2.5 rounded-full text-[12px] font-medium',
              tab === id ? 'bg-shodh-raised-2 text-shodh-text' : 'text-shodh-text-muted hover:text-shodh-text',
              FOCUS_RING,
            )}
          >
            {id === 'paste' ? 'Paste a config' : 'Fill in a form'}
          </button>
        ))}
      </div>
      {tab === 'paste' ? (
        <div className="flex flex-col gap-1.5">
          <label htmlFor={`${ids}-paste`} className="text-[12.5px] text-shodh-text-secondary">
            An <code>mcp.json</code> from Cursor or Claude Desktop, or just its servers
          </label>
          <textarea
            id={`${ids}-paste`}
            value={pasted}
            onChange={e => setPasted(e.target.value)}
            rows={6}
            spellCheck={false}
            placeholder={'{\n  "mcpServers": {\n    "github": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"] }\n  }\n}'}
            className={cn(INPUT, 'font-mono text-[12px] resize-y')}
          />
        </div>
      ) : (
        <div className="grid grid-cols-[120px_1fr] gap-x-3 gap-y-2 items-center">
          <label htmlFor={`${ids}-name`} className="text-[12.5px] text-shodh-text-secondary">Name</label>
          <input id={`${ids}-name`} value={form.name} onChange={e => setForm({ ...form, name: e.target.value })} className={INPUT} />
          <label htmlFor={`${ids}-kind`} className="text-[12.5px] text-shodh-text-secondary">Runs as</label>
          <select
            id={`${ids}-kind`}
            value={form.kind}
            onChange={e => setForm({ ...form, kind: e.target.value as ServerForm['kind'] })}
            className={INPUT}
          >
            <option value="stdio">A program on this computer</option>
            <option value="http">A server at a URL</option>
          </select>
          <label htmlFor={`${ids}-target`} className="text-[12.5px] text-shodh-text-secondary">
            {form.kind === 'stdio' ? 'Command' : 'URL'}
          </label>
          <input
            id={`${ids}-target`}
            value={form.target}
            onChange={e => setForm({ ...form, target: e.target.value })}
            placeholder={form.kind === 'stdio' ? 'npx -y @modelcontextprotocol/server-filesystem C:/Notes' : 'https://example.com/mcp'}
            spellCheck={false}
            className={cn(INPUT, 'font-mono text-[12px]')}
          />
          <label htmlFor={`${ids}-secrets`} className="text-[12.5px] text-shodh-text-secondary self-start pt-1.5">
            {form.kind === 'stdio' ? 'Environment' : 'Headers'}
          </label>
          <textarea
            id={`${ids}-secrets`}
            value={form.secrets}
            onChange={e => setForm({ ...form, secrets: e.target.value })}
            rows={2}
            spellCheck={false}
            placeholder={form.kind === 'stdio' ? 'API_TOKEN=…' : 'Authorization: Bearer …'}
            className={cn(INPUT, 'font-mono text-[12px] resize-y')}
          />
        </div>
      )}
      <p className="m-0 text-[12px] text-shodh-text-muted">
        Tokens stay in the config file on this computer. They are never shown to the model or written to logs.
      </p>
      {problem && (
        <p role="alert" className="m-0 text-[12px] text-shodh-error break-words">
          {problem}
        </p>
      )}
      <div>
        <button type="submit" disabled={saving} className={cn(BUTTON, 'bg-shodh-raised-2')}>
          {saving ? 'Adding…' : 'Add'}
        </button>
      </div>
    </form>
  );
}

function RepoLink({ repo }: { repo: string }) {
  return (
    <button
      type="button"
      onClick={() => void openUrl(repo).catch(e => notify.error('The link could not be opened', { description: errorText(e) }))}
      className={cn('text-shodh-accent-text hover:underline rounded', FOCUS_RING)}
    >
      source repo
    </button>
  );
}

function SkillsSection({ overview, workspaceId, onChanged }: { overview: ToolsOverview; workspaceId: string | null; onChanged: () => void }) {
  const [source, setSource] = useState('');
  const [staged, setStaged] = useState<StagedInstall | null>(null);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const sourceId = useId();

  const review = async (recommendedId?: string) => {
    setBusy(true);
    setProblem(null);
    try {
      setStaged(await (recommendedId ? toolsApi.prepareRecommended(recommendedId) : toolsApi.prepareSkills(source)));
    } catch (e) {
      setProblem(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const install = async () => {
    if (!staged) return;
    setBusy(true);
    try {
      const names = await toolsApi.confirmSkills(staged.token);
      notify.success(`Installed ${names.join(', ')}`);
      setStaged(null);
      setSource('');
      onChanged();
    } catch (e) {
      setProblem(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section aria-labelledby={`${sourceId}-heading`} className="flex flex-col gap-3">
      <div>
        <h3 id={`${sourceId}-heading`} className={SECTION_HEADING}>Skills</h3>
        <p className="m-0 mt-0.5 text-[12.5px] text-shodh-text-muted">
          Instructions the assistant loads when a task fits (the Agent Skills format: a folder with a SKILL.md). Only
          each skill's name and description are in every answer; files a skill ships are read, never run.
        </p>
      </div>
      {overview.skills.length > 0 && (
        <ul className="m-0 p-0 list-none flex flex-col gap-2">
          {overview.skills.map(skill => (
            <li key={skill.name} className="flex items-start gap-3 rounded-xl border border-shodh-border-subtle bg-shodh-surface-2 p-3">
              <div className="min-w-0 flex-1">
                <p className="m-0 text-[13px] font-semibold text-shodh-text">{skill.name}</p>
                <p className="m-0 text-[12px] text-shodh-text-muted">{skill.description}</p>
                <p className="m-0 mt-0.5 text-[11.5px] text-shodh-text-faint">
                  {skill.files === 1 ? '1 file' : `${skill.files} files`} besides SKILL.md
                  {skill.license && skill.repo && (
                    <>
                      {' · '}
                      {skill.license}
                      {' · '}
                      <RepoLink repo={skill.repo} />
                    </>
                  )}
                </p>
              </div>
              <label className="sr-only" htmlFor={`${sourceId}-${skill.name}-mode`}>Modes for {skill.name}</label>
              <select
                id={`${sourceId}-${skill.name}-mode`}
                value={modeChoice(skill.modes)}
                onChange={e => {
                  const choice = e.target.value as ModeChoice;
                  const modes: ToolMode[] = choice === 'both' ? [] : [choice];
                  void toolsApi
                    .setSkillModes(skill.name, modes)
                    .then(onChanged)
                    .catch(err => notify.error('The modes were not saved', { description: errorText(err) }));
                }}
                className={cn('h-8 rounded-lg border border-shodh-border bg-shodh-ground px-2 text-[12.5px] text-shodh-text', FOCUS_RING)}
              >
                <option value="both">Research and Code</option>
                <option value="research">Research only</option>
                <option value="code">Code only</option>
              </select>
              <Toggle
                label={`Use ${skill.name}${workspaceId ? ' in this workspace' : ''}`}
                checked={skill.enabled}
                onChange={enabled =>
                  void toolsApi
                    .setSkill(workspaceId, skill.name, enabled)
                    .then(onChanged)
                    .catch(e => notify.error('The skill was not changed', { description: errorText(e) }))
                }
              />
              <button
                type="button"
                aria-label={`Remove ${skill.name}`}
                title="Remove this skill from Shodh"
                onClick={() =>
                  void toolsApi
                    .removeSkill(skill.name)
                    .then(onChanged)
                    .catch(e => notify.error('The skill was not removed', { description: errorText(e) }))
                }
                className={cn('w-8 h-8 inline-flex items-center justify-center rounded-lg text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text', FOCUS_RING)}
              >
                <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
              </button>
            </li>
          ))}
        </ul>
      )}
      {overview.recommended.some(r => !r.installed) && (
        <div className="flex flex-col gap-2">
          <h4 className="m-0 text-[11.5px] font-semibold uppercase tracking-wide text-shodh-text-faint">Recommended</h4>
          <ul className="m-0 p-0 list-none flex flex-col gap-2">
            {overview.recommended
              .filter(r => !r.installed)
              .map(r => (
                <li key={r.id} className="flex items-start gap-3 rounded-xl border border-dashed border-shodh-border p-3">
                  <div className="min-w-0 flex-1">
                    <p className="m-0 text-[13px] font-semibold text-shodh-text">{r.title}</p>
                    <p className="m-0 text-[12px] text-shodh-text-muted">{r.description}</p>
                    <p className="m-0 mt-0.5 text-[11.5px] text-shodh-text-faint">
                      {r.skills.join(', ')} · {r.modes.length === 1 ? `${r.modes[0] === 'code' ? 'Code' : 'Research'} mode` : 'Research and Code'}
                      {' · '}
                      {r.license}
                      {' · '}
                      <RepoLink repo={r.repo} />
                    </p>
                  </div>
                  <button type="button" className={BUTTON} disabled={busy} onClick={() => void review(r.id)}>
                    Review
                  </button>
                </li>
              ))}
          </ul>
        </div>
      )}
      {overview.skillProblems.map(p => (
        <p key={p.folder} className="m-0 text-[12px] text-shodh-error break-words">
          {p.folder}: {p.error}
        </p>
      ))}
      <div className="flex flex-col gap-1.5">
        <label htmlFor={sourceId} className="text-[12.5px] text-shodh-text-secondary">Install from a folder or an https git URL</label>
        <div className="flex gap-2">
          <input
            id={sourceId}
            value={source}
            onChange={e => setSource(e.target.value)}
            placeholder="https://github.com/… or C:\Skills\my-skill"
            spellCheck={false}
            className={cn(INPUT, 'font-mono text-[12px]')}
          />
          <button
            type="button"
            className={BUTTON}
            onClick={() =>
              void pickFolder({ directory: true, multiple: false }).then(picked => {
                if (typeof picked === 'string') setSource(picked);
              })
            }
          >
            Browse
          </button>
          <button type="button" className={BUTTON} disabled={busy || !source.trim()} onClick={() => void review()}>
            {busy && !staged ? 'Reading…' : 'Review'}
          </button>
        </div>
      </div>
      {problem && (
        <p role="alert" className="m-0 text-[12px] text-shodh-error break-words">
          {problem}
        </p>
      )}
      {staged && (
        <div className="rounded-xl border border-shodh-border bg-shodh-surface-2 p-3.5 flex flex-col gap-2" aria-live="polite">
          <p className="m-0 text-[12.5px] text-shodh-text">
            {staged.skills.length === 1 ? 'This skill' : `These ${staged.skills.length} skills`} will be installed from{' '}
            <span className="font-mono break-all">{staged.source}</span>:
          </p>
          <ul className="m-0 pl-4 flex flex-col gap-1 text-[12.5px]">
            {staged.skills.map(s => (
              <li key={s.name}>
                <span className="font-semibold text-shodh-text">{s.name}</span>
                <span className="text-shodh-text-muted"> · {s.description} · {s.files} files</span>
                {s.replaces && <span className="text-shodh-warning"> · replaces the installed one</span>}
              </li>
            ))}
          </ul>
          {staged.problems.length > 0 && (
            <p className="m-0 text-[12px] text-shodh-text-muted">{staged.problems.length} folder(s) could not be read and are skipped.</p>
          )}
          <div className="flex gap-2">
            <button type="button" className={cn(BUTTON, 'bg-shodh-raised-2')} disabled={busy} onClick={() => void install()}>
              {busy ? 'Installing…' : 'Install'}
            </button>
            <button
              type="button"
              className={BUTTON}
              disabled={busy}
              onClick={() => {
                void toolsApi.cancelSkills(staged.token);
                setStaged(null);
              }}
            >
              Cancel
            </button>
          </div>
        </div>
      )}
    </section>
  );
}

function formatUsed(when: string | null): string {
  if (!when) return 'Never used';
  const date = new Date(when);
  return Number.isNaN(date.getTime()) ? 'Used' : `Used ${date.toLocaleDateString()}`;
}

/**
 * Settings → Tools & connections: the built-in tools, MCP servers (global or
 * per workspace) with per-tool approval, and skills.
 */
export default function ToolsSettings() {
  const { workspaces } = useWorkspaces();
  const [workspaceId, setWorkspaceId] = useState<string | null>(null);
  const [overview, setOverview] = useState<ToolsOverview | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [installingEnola, setInstallingEnola] = useState(false);
  const [builtinOpen, setBuiltinOpen] = useState(false);
  const generation = useRef(0);
  const scopeId = useId();

  const load = useCallback(() => {
    const run = ++generation.current;
    toolsApi
      .overview(workspaceId)
      .then(next => {
        if (run !== generation.current) return;
        setOverview(next);
        setLoadError(null);
      })
      .catch(e => {
        if (run === generation.current) setLoadError(errorText(e));
      });
  }, [workspaceId]);

  useEffect(() => load(), [load]);

  // Start the enabled servers once so their status and tools show.
  const probed = useRef(new Set<string>());
  useEffect(() => {
    if (!overview) return;
    for (const server of overview.servers) {
      const key = `${workspaceId ?? ''}/${server.name}`;
      if (!server.enabled || server.status !== 'unknown' || probed.current.has(key)) continue;
      probed.current.add(key);
      toolsApi
        .testServer(workspaceId, server.name)
        .then(tested => setOverview(o => (o ? { ...o, servers: o.servers.map(s => (s.name === tested.name ? tested : s)) } : o)))
        .catch(() => undefined);
    }
  }, [overview, workspaceId]);

  const installEnola = async () => {
    setInstallingEnola(true);
    try {
      await toolsApi.installEnola();
      notify.success('enola is ready', { description: 'It runs in Code mode chats, in the workspace’s code folder.' });
      load();
    } catch (e) {
      notify.error('enola could not be added', { description: errorText(e) });
    } finally {
      setInstallingEnola(false);
    }
  };

  const openFile = async () => {
    try {
      await toolsApi.openConfig(workspaceId);
    } catch (e) {
      notify.error('The config file could not be opened', { description: errorText(e) });
    }
  };

  if (loadError) return <p role="alert" className="m-0 text-[13px] text-shodh-error">{loadError}</p>;
  if (!overview) return <p className="m-0 text-[13px] text-shodh-text-muted">Loading…</p>;

  const enola = overview.enola;
  return (
    <div className="flex flex-col gap-7">
      <div className="flex flex-col gap-1.5">
        <label htmlFor={scopeId} className="text-[12.5px] font-medium text-shodh-text-secondary">
          Servers and skills for
        </label>
        <select
          id={scopeId}
          value={workspaceId ?? ''}
          onChange={e => {
            setOverview(null);
            setAdding(false);
            setWorkspaceId(e.target.value || null);
          }}
          className={cn(INPUT, 'max-w-[360px]')}
        >
          <option value="">Every chat</option>
          {workspaces.map(w => (
            <option key={w.id} value={w.id}>
              The workspace {w.name}
            </option>
          ))}
        </select>
        <p className="m-0 text-[12px] text-shodh-text-muted">
          {workspaceId
            ? 'Servers here are added to this workspace’s chats and replace an every-chat server of the same name.'
            : 'Servers here are offered in every chat, in the modes you choose.'}
        </p>
      </div>

      <section aria-labelledby={`${scopeId}-mcp`} className="flex flex-col gap-3">
        <div className="flex items-start gap-3 flex-wrap">
          <div className="min-w-0 flex-1">
            <h3 id={`${scopeId}-mcp`} className={SECTION_HEADING}>MCP servers</h3>
            <p className="m-0 mt-0.5 text-[12.5px] text-shodh-text-muted">
              Programs and services that give the assistant more tools. Tools the server does not mark read-only ask
              you before every call.
            </p>
          </div>
          <button type="button" className={BUTTON} onClick={() => setAdding(a => !a)} aria-expanded={adding}>
            <Plus className="w-3.5 h-3.5" aria-hidden="true" />
            Add server
          </button>
          <button type="button" className={BUTTON} onClick={() => void openFile()} title={overview.configPath}>
            <FileJson className="w-3.5 h-3.5" aria-hidden="true" />
            Edit as file
          </button>
        </div>
        {adding && (
          <AddServer
            workspaceId={workspaceId}
            onAdded={() => {
              setAdding(false);
              load();
            }}
          />
        )}
        {overview.configError && <p role="alert" className="m-0 text-[12px] text-shodh-error break-words">{overview.configError}</p>}
        {overview.problems.map(p => (
          <p key={p.name} className="m-0 text-[12px] text-shodh-error break-words">
            {p.name ? `${p.name}: ` : ''}
            {p.error}
          </p>
        ))}
        {overview.servers.length === 0 && !adding && (
          <p className="m-0 text-[12.5px] text-shodh-text-muted">No servers yet.</p>
        )}
        <ul className="m-0 p-0 list-none flex flex-col gap-2.5">
          {overview.servers.map(server => (
            <ServerCard
              key={server.name}
              server={server}
              workspaceId={workspaceId}
              onChanged={load}
              onTested={tested =>
                setOverview(o => (o ? { ...o, servers: o.servers.map(s => (s.name === tested.name ? tested : s)) } : o))
              }
            />
          ))}
        </ul>
        {!workspaceId && enola.supported && !enola.registered && (
          <div className="rounded-xl border border-dashed border-shodh-border p-3.5 flex items-center gap-3">
            <div className="min-w-0 flex-1">
              <p className="m-0 text-[13px] font-semibold text-shodh-text">enola {enola.version}</p>
              <p className="m-0 text-[12px] text-shodh-text-muted">
                Maps a codebase’s architecture (modules, dependencies, impact of a change) for Code mode. Downloaded
                from its GitHub release and checked against a pinned checksum; it reads the code folder and keeps its
                index in <code>.enola/</code> there.
              </p>
            </div>
            <button type="button" className={BUTTON} disabled={installingEnola} onClick={() => void installEnola()}>
              {installingEnola ? 'Adding…' : 'Add enola'}
            </button>
          </div>
        )}
      </section>

      <SkillsSection overview={overview} workspaceId={workspaceId} onChanged={load} />

      <section aria-labelledby={`${scopeId}-builtin`} className="flex flex-col gap-3">
        <div>
          <h3 id={`${scopeId}-builtin`} className={SECTION_HEADING}>Built-in tools</h3>
          <p className="m-0 mt-0.5 text-[12.5px] text-shodh-text-muted">
            What Shodh’s assistant can always do. Tools that change something ask you first.
          </p>
        </div>
        <button
          type="button"
          onClick={() => setBuiltinOpen(o => !o)}
          aria-expanded={builtinOpen}
          className={cn('self-start inline-flex items-center gap-1 text-[12.5px] text-shodh-text-secondary hover:text-shodh-text rounded', FOCUS_RING)}
        >
          {builtinOpen ? <ChevronDown className="w-3.5 h-3.5" aria-hidden="true" /> : <ChevronRight className="w-3.5 h-3.5" aria-hidden="true" />}
          {overview.builtin.reduce((n, g) => n + g.tools.length, 0)} tools in {overview.builtin.length} groups
        </button>
        {builtinOpen &&
          overview.builtin.map(group => (
            <div key={group.group} className="flex flex-col gap-1">
              <h4 className="m-0 text-[11.5px] font-semibold uppercase tracking-wide text-shodh-text-faint">{group.group}</h4>
              <ul className="m-0 p-0 list-none flex flex-col">
                {group.tools.map(tool => (
                  <li key={tool.name} className="flex items-baseline gap-3 py-1 text-[12.5px]">
                    <span className="font-mono text-shodh-text shrink-0">{tool.name}</span>
                    <span className="text-shodh-text-muted truncate flex-1" title={tool.description}>
                      {tool.description}
                    </span>
                    {tool.tier !== 'read' && (
                      <span className="text-[11px] text-shodh-text-faint shrink-0">{tool.tier === 'destructive' ? 'always asks' : 'asks'}</span>
                    )}
                    <span className="text-[11px] text-shodh-text-faint shrink-0">{formatUsed(tool.lastUsed)}</span>
                  </li>
                ))}
              </ul>
            </div>
          ))}
      </section>
    </div>
  );
}
