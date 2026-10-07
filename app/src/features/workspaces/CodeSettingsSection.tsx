import { useEffect, useId, useState } from 'react';
import { Loader2, Save } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { agentApi, toAgentError } from '../agent/useAgentSession';
import type { ApprovalLevel, CodeSettings } from '../agent/useAgentSession';
import { FOCUS_RING, LABEL, OUTLINE_BUTTON, SECTION_TITLE, TEXTAREA } from './ui';

export const APPROVAL_LEVELS: readonly { id: ApprovalLevel; label: string; hint: string }[] = [
  { id: 'askEveryTime', label: 'Ask every time', hint: 'Every edit, file and command waits for you.' },
  {
    id: 'autoApplyEdits',
    label: 'Auto-apply edits',
    hint: "Edits and new files apply by themselves on the conversation's branch (Discard changes undoes them). Commands still ask.",
  },
  { id: 'trusted', label: 'Trusted', hint: 'Edits apply by themselves, and commands starting with one of the lines below run without asking.' },
];

const linesOf = (text: string) => text.split('\n').map(l => l.trim()).filter(Boolean);

/**
 * How much of Code mode's work in this workspace runs without asking. Read
 * by every running Code session at its next change.
 */
export function CodeSettingsSection({ workspaceId }: { workspaceId: string }) {
  const ids = { title: useId(), allowlist: useId(), help: useId() };
  const [saved, setSaved] = useState<CodeSettings | null>(null);
  const [level, setLevel] = useState<ApprovalLevel>('askEveryTime');
  const [allowlist, setAllowlist] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let active = true;
    setSaved(null);
    setError(null);
    agentApi
      .codeSettings(workspaceId)
      .then(settings => {
        if (!active) return;
        setSaved(settings);
        setLevel(settings.approval);
        setAllowlist(settings.allowlist.join('\n'));
      })
      .catch(err => { if (active) setError(toAgentError(err).message); });
    return () => { active = false; };
  }, [workspaceId]);

  const dirty = saved !== null && (level !== saved.approval || linesOf(allowlist).join('\n') !== saved.allowlist.join('\n'));

  const save = async () => {
    if (!dirty) return;
    setSaving(true);
    try {
      const next = await agentApi.setCodeSettings(workspaceId, { approval: level, allowlist: linesOf(allowlist) });
      setSaved(next);
      setLevel(next.approval);
      setAllowlist(next.allowlist.join('\n'));
      notify.success('Code mode approvals saved');
    } catch (err) {
      notify.error('Code mode approvals were not saved', { description: toAgentError(err).message });
    } finally {
      setSaving(false);
    }
  };

  return (
    <section aria-labelledby={ids.title} className="flex flex-col gap-2">
      <div>
        <h3 id={ids.title} className={SECTION_TITLE}>Code mode approvals</h3>
        <p id={ids.help} className="m-0 text-[12.5px] text-shodh-text-muted">
          Commands that delete files, reset or rewrite git history, or push always ask, whatever you choose.
        </p>
      </div>
      {error && <p role="alert" className="m-0 text-[12.5px] text-shodh-error">The settings could not be read: {error}</p>}
      <form
        className="flex flex-col gap-3"
        aria-busy={saved === null && !error}
        onSubmit={e => {
          e.preventDefault();
          void save();
        }}
      >
        <fieldset className="m-0 p-0 border-0 flex flex-col gap-1" aria-describedby={ids.help} disabled={saved === null}>
          <legend className="sr-only">When Code mode asks</legend>
          {APPROVAL_LEVELS.map(option => (
            <label
              key={option.id}
              className={cn(
                'flex items-start gap-2.5 px-3 py-2 rounded-lg border cursor-pointer transition-colors duration-micro',
                level === option.id ? 'border-shodh-border-strong bg-shodh-raised' : 'border-transparent hover:bg-shodh-raised',
              )}
            >
              <input
                type="radio"
                name={`${ids.title}-level`}
                value={option.id}
                checked={level === option.id}
                onChange={() => setLevel(option.id)}
                className={cn('mt-[3px] accent-shodh-accent', FOCUS_RING)}
              />
              <span className="flex flex-col">
                <span className="text-[13px] font-medium text-shodh-text">{option.label}</span>
                <span className="text-[12px] text-shodh-text-muted">{option.hint}</span>
              </span>
            </label>
          ))}
        </fieldset>
        {level === 'trusted' && (
          <div className="flex flex-col gap-1">
            <label htmlFor={ids.allowlist} className={LABEL}>Commands that run without asking, one per line</label>
            <textarea
              id={ids.allowlist}
              rows={4}
              value={allowlist}
              onChange={e => setAllowlist(e.target.value)}
              spellCheck={false}
              placeholder="cargo test"
              className={cn(TEXTAREA, 'font-mono text-[12.5px]')}
            />
            <p className="m-0 text-[12px] text-shodh-text-muted">
              A command runs by itself when its words start with a line here and it has no ; | &amp; &gt; $ or backticks. Tests run code the assistant may have changed.
            </p>
          </div>
        )}
        <div>
          <button type="submit" className={OUTLINE_BUTTON} disabled={!dirty || saving}>
            {saving ? <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <Save className="w-3.5 h-3.5" aria-hidden="true" />}
            Save approvals
          </button>
        </div>
      </form>
    </section>
  );
}
