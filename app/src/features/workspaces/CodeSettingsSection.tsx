import { useEffect, useId, useState } from 'react';
import { Loader2, Save } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { agentApi, toAgentError } from '../agent/useAgentSession';
import type { ApprovalLevel, CodeSettings } from '../agent/useAgentSession';
import { FOCUS_RING, INPUT, LABEL, OUTLINE_BUTTON, SECTION_TITLE, TEXTAREA } from './ui';
import { desktopPlatform } from '../../lib/startupText';
import { COMMAND_LIMITS, commandLimitHint, commandSandboxText, parseCommandLimit } from './commandLimits';
import type { CommandLimitKey } from './commandLimits';

export const APPROVAL_LEVELS: readonly { id: ApprovalLevel; label: string; hint: string }[] = [
  { id: 'askEveryTime', label: 'Ask every time', hint: 'Every edit, file and command waits for you.' },
  {
    id: 'autoApplyEdits',
    label: 'Auto-apply edits',
    hint: "Edits and new files apply by themselves on the conversation's branch (Discard changes undoes them). Files git ignores, which Discard cannot restore, and files that can run programs (.vscode, .envrc, package.json, build and test settings such as Cargo.toml, build.rs, conftest.py, pyproject.toml, Makefile) still ask, and so do commands.",
  },
  {
    id: 'trusted',
    label: 'Trusted',
    hint: 'As Auto-apply edits, and a command that is exactly one of the lines below runs without asking, unless files changed without your review (edits, or what other commands and tools wrote): then it asks and lists them, and approving it in the conversation marks them reviewed.',
  },
];

const linesOf = (text: string) => text.split('\n').map(l => l.trim()).filter(Boolean);

/** The command sandbox on this system, in words. */
const SANDBOX_TEXT = commandSandboxText(desktopPlatform(typeof navigator !== 'undefined' ? navigator.platform : ''));

type LimitTexts = Record<CommandLimitKey, string>;

/** The saved limits as the fields show them (a default for one an older backend left out). */
function limitTexts(settings: CodeSettings): LimitTexts {
  const out = {} as LimitTexts;
  for (const spec of COMMAND_LIMITS) {
    const value = settings[spec.key];
    out[spec.key] = String(typeof value === 'number' && Number.isFinite(value) ? value : spec.defaultValue);
  }
  return out;
}

const EMPTY_LIMITS: LimitTexts = { commandTimeoutSecs: '', memoryLimitMb: '', cpuPercent: '' };

/**
 * How much of Code mode's work in this workspace runs without asking. Read
 * by every running Code session at its next change.
 */
export function CodeSettingsSection({ workspaceId }: { workspaceId: string }) {
  const ids = { title: useId(), allowlist: useId(), help: useId(), limits: useId(), limitsHelp: useId() };
  const [saved, setSaved] = useState<CodeSettings | null>(null);
  const [level, setLevel] = useState<ApprovalLevel>('askEveryTime');
  const [allowlist, setAllowlist] = useState('');
  const [limits, setLimits] = useState<LimitTexts>(EMPTY_LIMITS);
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
        setLimits(limitTexts(settings));
      })
      .catch(err => { if (active) setError(toAgentError(err).message); });
    return () => { active = false; };
  }, [workspaceId]);

  const parsedLimits = COMMAND_LIMITS.map(spec => ({ spec, parsed: parseCommandLimit(spec, limits[spec.key]) }));
  const limitsValid = parsedLimits.every(({ parsed }) => parsed.ok);
  const savedLimits = saved ? limitTexts(saved) : null;
  const limitsChanged = savedLimits !== null && COMMAND_LIMITS.some(spec => limits[spec.key].trim() !== savedLimits[spec.key]);
  const dirty = saved !== null
    && (level !== saved.approval || linesOf(allowlist).join('\n') !== saved.allowlist.join('\n') || limitsChanged);

  const save = async () => {
    if (!dirty) return;
    const values = {} as Record<CommandLimitKey, number>;
    for (const { spec, parsed } of parsedLimits) {
      if (parsed.ok === false) return;
      values[spec.key] = parsed.value;
    }
    setSaving(true);
    try {
      const next = await agentApi.setCodeSettings(workspaceId, {
        approval: level,
        allowlist: linesOf(allowlist),
        commandTimeoutSecs: values.commandTimeoutSecs,
        memoryLimitMb: values.memoryLimitMb,
        cpuPercent: values.cpuPercent,
      });
      setSaved(next);
      setLevel(next.approval);
      setAllowlist(next.allowlist.join('\n'));
      setLimits(limitTexts(next));
      notify.success('Code mode settings saved');
    } catch (err) {
      notify.error('Code mode settings were not saved', { description: toAgentError(err).message });
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
              A command runs by itself only when it is exactly a line here, with nothing added, and has no ; | &amp; &gt; $ or backticks. It also needs no other options, must run in the code folder itself, and asks while another command is running. Tests run code the assistant may have changed, so after a change you did not review, the command asks first.
            </p>
          </div>
        )}
        <fieldset
          className="m-0 p-0 border-0 flex flex-col gap-2"
          aria-describedby={ids.limitsHelp}
          disabled={saved === null}
        >
          <legend className={cn(SECTION_TITLE, 'p-0 mb-0.5')}>Command limits</legend>
          <p id={ids.limitsHelp} className="m-0 text-[12.5px] text-shodh-text-muted">
            Each command runs in its own process under these limits, together with everything it starts, and may have 64 processes at once. A command still running at the time limit is stopped. Beyond the memory or process limit, Windows refuses what is asked for and macOS stops the command. macOS has no CPU cap: commands there run at a lower priority instead.
          </p>
          {parsedLimits.map(({ spec, parsed }) => {
            const inputId = `${ids.limits}-${spec.key}`;
            const errorId = `${inputId}-error`;
            const hintId = `${inputId}-hint`;
            const invalid = saved !== null && parsed.ok === false;
            const hint = parsed.ok === true ? commandLimitHint(spec.key, parsed.value) : null;
            const describedBy = [invalid ? errorId : null, hint ? hintId : null].filter(Boolean).join(' ') || undefined;
            return (
              <div key={spec.key} className="flex flex-col gap-1">
                <label htmlFor={inputId} className={LABEL}>{`${spec.label} (${spec.unit})`}</label>
                <div className="flex items-center gap-2">
                  <input
                    id={inputId}
                    type="number"
                    inputMode="numeric"
                    min={spec.min}
                    max={spec.max}
                    step={1}
                    value={limits[spec.key]}
                    onChange={e => {
                      const text = e.target.value;
                      setLimits(current => ({ ...current, [spec.key]: text }));
                    }}
                    aria-invalid={invalid || undefined}
                    aria-describedby={describedBy}
                    className={cn(INPUT, 'w-36 font-mono text-[12.5px]', invalid && 'border-shodh-error')}
                  />
                  {hint && <span id={hintId} className="text-[12px] text-shodh-text-muted">{hint}</span>}
                </div>
                {invalid && parsed.ok === false && (
                  <p id={errorId} className="m-0 text-[12px] text-shodh-error">{parsed.error}</p>
                )}
              </div>
            );
          })}
        </fieldset>
        <div className="flex flex-col gap-0.5">
          <h4 className={SECTION_TITLE}>Command sandbox</h4>
          <p className="m-0 text-[12.5px] text-shodh-text-muted">{SANDBOX_TEXT}</p>
        </div>
        <div>
          <button type="submit" className={OUTLINE_BUTTON} disabled={!dirty || !limitsValid || saving}>
            {saving ? <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <Save className="w-3.5 h-3.5" aria-hidden="true" />}
            Save Code settings
          </button>
        </div>
      </form>
    </section>
  );
}
