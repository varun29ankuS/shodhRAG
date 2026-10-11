import React, { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { History, Loader2, RotateCcw, Save } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { formatDate } from '../memory/model';
import { workspaceError, workspacesApi } from './api';
import { DiffView } from './DiffView';
import { lineDiff, MAX_INSTRUCTIONS_CHARS, WORKSPACE_COLORS, WORKSPACE_ICONS } from './model';
import type { InstructionVersion, WorkspaceDetail } from './types';
import { WorkspaceIcon } from './WorkspaceIcon';
import { CodeSettingsSection } from './CodeSettingsSection';
import { FOCUS_RING, INPUT, LABEL, OUTLINE_BUTTON, PRIMARY_BUTTON, QUIET_BUTTON, SECTION_TITLE, TEXTAREA } from './ui';

const AUTHOR_LABELS: Record<InstructionVersion['author'], string> = {
  user: 'You',
  agent: 'Shodh (you approved)',
  template: 'Template',
  migration: 'Import',
};

/** Name, description, icon and colour of a workspace, saved together. */
function DetailsForm({ workspace, onSaved }: { workspace: WorkspaceDetail; onSaved: () => void }) {
  const ids = { name: useId(), description: useId(), icon: useId(), color: useId() };
  const [name, setName] = useState(workspace.name);
  const [description, setDescription] = useState(workspace.description);
  const [icon, setIcon] = useState(workspace.icon);
  const [color, setColor] = useState(workspace.color);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    setName(workspace.name);
    setDescription(workspace.description);
    setIcon(workspace.icon);
    setColor(workspace.color);
  }, [workspace.id, workspace.name, workspace.description, workspace.icon, workspace.color]);

  const dirty = name.trim() !== workspace.name || description.trim() !== workspace.description || icon !== workspace.icon || color !== workspace.color;

  const save = async () => {
    if (!dirty || !name.trim()) return;
    setSaving(true);
    try {
      await workspacesApi.update(workspace.id, { name: name.trim(), description: description.trim(), icon, color });
      notify.success('Workspace saved');
      onSaved();
    } catch (err) {
      notify.error('The workspace was not saved', { description: workspaceError(err).message });
    } finally {
      setSaving(false);
    }
  };

  return (
    <form
      className="flex flex-col gap-3"
      onSubmit={e => {
        e.preventDefault();
        void save();
      }}
    >
      <div className="grid grid-cols-1 md:grid-cols-[1fr_1fr] gap-3">
        <div className="flex flex-col gap-1">
          <label htmlFor={ids.name} className={LABEL}>Name</label>
          <input id={ids.name} className={INPUT} value={name} maxLength={80} onChange={e => setName(e.target.value)} required />
        </div>
        <div className="flex flex-col gap-1">
          <label htmlFor={ids.description} className={LABEL}>Description</label>
          <input
            id={ids.description}
            className={INPUT}
            value={description}
            maxLength={500}
            onChange={e => setDescription(e.target.value)}
            placeholder="What this work is about"
          />
        </div>
      </div>
      <div className="flex flex-wrap items-start gap-6">
        <fieldset className="m-0 p-0 border-0 flex flex-col gap-1.5">
          <legend className={cn(LABEL, 'mb-1.5')}>Icon</legend>
          <div role="radiogroup" aria-label="Icon" className="flex flex-wrap gap-1">
            {WORKSPACE_ICONS.map(i => (
              <button
                key={i}
                type="button"
                role="radio"
                aria-checked={icon === i}
                aria-label={i.replace(/-/g, ' ')}
                onClick={() => setIcon(i)}
                className={cn(
                  'w-8 h-8 rounded-lg inline-flex items-center justify-center border transition-colors duration-micro',
                  icon === i ? 'border-shodh-accent bg-shodh-accent-soft' : 'border-transparent hover:bg-shodh-raised',
                  FOCUS_RING,
                )}
              >
                <WorkspaceIcon icon={i} color={color} className="w-4 h-4" />
              </button>
            ))}
          </div>
        </fieldset>
        <fieldset className="m-0 p-0 border-0 flex flex-col gap-1.5">
          <legend className={cn(LABEL, 'mb-1.5')}>Colour</legend>
          <div role="radiogroup" aria-label="Colour" className="flex flex-wrap gap-1">
            {WORKSPACE_COLORS.map(c => (
              <button
                key={c}
                type="button"
                role="radio"
                aria-checked={color === c}
                aria-label={c}
                onClick={() => setColor(c)}
                className={cn(
                  'w-8 h-8 rounded-lg inline-flex items-center justify-center border transition-colors duration-micro',
                  color === c ? 'border-shodh-accent bg-shodh-accent-soft' : 'border-transparent hover:bg-shodh-raised',
                  FOCUS_RING,
                )}
              >
                <WorkspaceIcon icon={icon} color={c} className="w-4 h-4" />
              </button>
            ))}
          </div>
        </fieldset>
      </div>
      <div>
        <button type="submit" className={OUTLINE_BUTTON} disabled={!dirty || saving || !name.trim()}>
          {saving ? <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <Save className="w-3.5 h-3.5" aria-hidden="true" />}
          Save details
        </button>
      </div>
    </form>
  );
}

/**
 * The instructions every answer in the workspace follows: an editor (saving makes a new
 * version, refused when someone saved a newer one meanwhile), and the version history
 * with a diff against the previous version and "Use this version".
 */
function InstructionsEditor({ workspace, onSaved }: { workspace: WorkspaceDetail; onSaved: () => void }) {
  const ids = { text: useId(), help: useId(), count: useId(), history: useId() };
  const [text, setText] = useState(workspace.instructions);
  // The version the editor's text started from; saving names it, so a save over a newer
  // version is refused unless the user saw the warning below.
  const [base, setBase] = useState({ version: workspace.instructionsVersion, text: workspace.instructions });
  const [saving, setSaving] = useState(false);
  const [stale, setStale] = useState(false);
  const [history, setHistory] = useState<InstructionVersion[] | null>(null);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [historyError, setHistoryError] = useState<string | null>(null);
  const [compare, setCompare] = useState<number | null>(null);
  const textRef = useRef(text);
  textRef.current = text;
  const baseRef = useRef(base);
  baseRef.current = base;
  const idRef = useRef(workspace.id);

  // A newer version arrived (saved here, or an assistant edit the user approved): the
  // editor takes it, unless it holds unsaved edits, which are kept and flagged.
  useEffect(() => {
    const switched = idRef.current !== workspace.id;
    idRef.current = workspace.id;
    const previous = baseRef.current;
    if (!switched && previous.version === workspace.instructionsVersion) return;
    const unedited = switched || textRef.current.trim() === previous.text.trim();
    setBase({ version: workspace.instructionsVersion, text: workspace.instructions });
    if (unedited) {
      setText(workspace.instructions);
      setStale(false);
    } else {
      setStale(true);
    }
  }, [workspace.id, workspace.instructionsVersion, workspace.instructions]);

  const loadHistory = useCallback(async () => {
    try {
      setHistory(await workspacesApi.history(workspace.id));
      setHistoryError(null);
    } catch (err) {
      setHistoryError(workspaceError(err).message);
    }
  }, [workspace.id]);

  useEffect(() => {
    if (historyOpen) void loadHistory();
  }, [historyOpen, loadHistory, workspace.instructionsVersion]);

  const count = text.length;
  const tooLong = count > MAX_INSTRUCTIONS_CHARS;
  const dirty = text.trim() !== workspace.instructions.trim();

  const save = async () => {
    if (!dirty || tooLong) return;
    setSaving(true);
    try {
      const version = await workspacesApi.setInstructions(workspace.id, text, base.version);
      setBase({ version: version.version, text: version.text });
      setStale(false);
      notify.success(`Instructions saved as version ${version.version}`);
      onSaved();
    } catch (err) {
      const e = workspaceError(err);
      if (e.code === 'stale') {
        setStale(true);
        notify.error('The instructions changed meanwhile', { description: 'Your text is kept here. Compare it with the current version, then save again.' });
        onSaved();
      } else {
        notify.error('The instructions were not saved', { description: e.message });
      }
    } finally {
      setSaving(false);
    }
  };

  const versions = history ?? [];
  const comparing = compare === null ? null : versions.find(v => v.version === compare) ?? null;
  const previous = comparing ? versions.find(v => v.version === comparing.version - 1) ?? null : null;
  const comparison = useMemo(
    () => (comparing ? lineDiff(previous?.text ?? '', comparing.text) : null),
    [comparing, previous],
  );

  return (
    <section aria-labelledby={`${ids.text}-title`} className="flex flex-col gap-2">
      <div className="flex items-end justify-between gap-3">
        <div>
          <h3 id={`${ids.text}-title`} className={SECTION_TITLE}>Instructions</h3>
          <p id={ids.help} className="m-0 text-[12.5px] text-shodh-text-muted">
            Sent with every question in this workspace, marked as your instructions. They say how to answer; sources
            still decide what is true. The assistant can propose changes, which you approve and can see as a diff.
          </p>
        </div>
      </div>
      {stale && (
        <p role="alert" className="m-0 rounded-lg border border-shodh-warning/50 bg-shodh-warning-soft px-3 py-2 text-[12.5px] text-shodh-text">
          A newer version (v{workspace.instructionsVersion}) was saved while you were editing. Saving now makes your text
          the next version; the history keeps both.
          <button
            type="button"
            className={cn(QUIET_BUTTON, 'ml-2 h-7')}
            onClick={() => {
              setText(workspace.instructions);
              setStale(false);
            }}
          >
            Discard my edits
          </button>
        </p>
      )}
      <label htmlFor={ids.text} className="sr-only">Instructions</label>
      <textarea
        id={ids.text}
        className={cn(TEXTAREA, 'min-h-[180px] font-normal', tooLong && 'border-shodh-error')}
        value={text}
        onChange={e => setText(e.target.value)}
        aria-describedby={`${ids.help} ${ids.count}`}
        aria-invalid={tooLong}
        placeholder="For example: Answer as a short report. Cite page numbers. Use British English."
        rows={8}
      />
      <div className="flex flex-wrap items-center gap-2">
        <button type="button" className={PRIMARY_BUTTON} onClick={() => void save()} disabled={!dirty || saving || tooLong}>
          {saving ? <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <Save className="w-4 h-4" aria-hidden="true" />}
          {stale ? 'Save as newest version' : 'Save instructions'}
        </button>
        {dirty && (
          <button type="button" className={QUIET_BUTTON} onClick={() => setText(workspace.instructions)}>
            <RotateCcw className="w-3.5 h-3.5" aria-hidden="true" /> Undo changes
          </button>
        )}
        <span id={ids.count} className={cn('ml-auto text-[12px]', tooLong ? 'text-shodh-error' : 'text-shodh-text-muted')}>
          {`${count.toLocaleString()} / ${MAX_INSTRUCTIONS_CHARS.toLocaleString()} characters`}
          {workspace.instructionsVersion > 0 ? ` · version ${workspace.instructionsVersion}` : ''}
        </span>
      </div>

      <div className="flex flex-col gap-2 pt-1">
        <button
          type="button"
          className={cn(QUIET_BUTTON, 'self-start')}
          aria-expanded={historyOpen}
          aria-controls={ids.history}
          onClick={() => setHistoryOpen(o => !o)}
          disabled={workspace.instructionsVersion === 0}
        >
          <History className="w-3.5 h-3.5" aria-hidden="true" />
          {workspace.instructionsVersion === 0 ? 'No versions yet' : historyOpen ? 'Hide version history' : 'Version history'}
        </button>
        {historyOpen && (
          <div id={ids.history} className="flex flex-col gap-2">
            {historyError ? (
              <p role="alert" className="m-0 text-[12.5px] text-shodh-error">History could not be loaded: {historyError}</p>
            ) : history === null ? (
              <p role="status" className="m-0 text-[12.5px] text-shodh-text-muted">Loading history…</p>
            ) : (
              <ol className="m-0 p-0 list-none divide-y divide-shodh-border-subtle rounded-lg border border-shodh-border-subtle">
                {versions.map(v => (
                  <li key={v.version} className="px-3 py-2 flex flex-col gap-1.5">
                    <div className="flex flex-wrap items-center gap-2 text-[12.5px]">
                      <span className="font-semibold text-shodh-text">Version {v.version}</span>
                      <span className="text-shodh-text-muted">{AUTHOR_LABELS[v.author]} · {formatDate(v.createdAt)}</span>
                      {v.version === workspace.instructionsVersion && (
                        <span className="text-[11px] px-1.5 py-0.5 rounded-full bg-shodh-accent-soft text-shodh-accent-text">Current</span>
                      )}
                      <span className="ml-auto flex items-center gap-1">
                        <button
                          type="button"
                          className={cn(QUIET_BUTTON, 'h-7')}
                          aria-pressed={compare === v.version}
                          onClick={() => setCompare(c => (c === v.version ? null : v.version))}
                        >
                          {compare === v.version ? 'Hide changes' : 'Show changes'}
                        </button>
                        {v.version !== workspace.instructionsVersion && (
                          <button
                            type="button"
                            className={cn(QUIET_BUTTON, 'h-7')}
                            onClick={() => {
                              setText(v.text);
                              notify.info(`Version ${v.version} is in the editor`, { description: 'Save to make it the newest version.' });
                            }}
                          >
                            Use this version
                          </button>
                        )}
                      </span>
                    </div>
                    {v.note && <p className="m-0 text-[12px] text-shodh-text-secondary">{v.note}</p>}
                    {compare === v.version && comparison && (
                      <DiffView
                        lines={comparison}
                        label={previous ? `Changes from version ${previous.version} to ${v.version}` : `Version ${v.version}`}
                      />
                    )}
                  </li>
                ))}
              </ol>
            )}
          </div>
        )}
      </div>
    </section>
  );
}

export function OverviewTab({ workspace, onChanged }: { workspace: WorkspaceDetail; onChanged: () => void }) {
  return (
    <div className="flex flex-col gap-8">
      <DetailsForm workspace={workspace} onSaved={onChanged} />
      <InstructionsEditor workspace={workspace} onSaved={onChanged} />
      <CodeSettingsSection workspaceId={workspace.id} />
    </div>
  );
}
