import { useEffect, useId, useState } from 'react';
import { Loader2, Save } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { agentApi, toAgentError } from '../agent/useAgentSession';
import type { CodeSettings } from '../agent/useAgentSession';
import { INPUT, LABEL, OUTLINE_BUTTON, SECTION_TITLE, TEXTAREA } from './ui';
import { desktopPlatform } from '../../lib/startupText';
import { COMMAND_LIMITS, commandLimitHint, commandSandboxText, parseCommandLimit } from './commandLimits';
import type { CommandLimitKey } from './commandLimits';

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
 * This workspace's Code mode settings: the commands Auto mode also runs without asking,
 * and the limits every command runs with. Read by every running Code session at its next
 * decision. What runs without asking is each conversation's mode, picked in the composer.
 */
export function CodeSettingsSection({ workspaceId }: { workspaceId: string }) {
  const ids = { title: useId(), allowlist: useId(), help: useId(), limits: useId(), limitsHelp: useId() };
  const [saved, setSaved] = useState<CodeSettings | null>(null);
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
    && (linesOf(allowlist).join('\n') !== saved.allowlist.join('\n') || limitsChanged);

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
        allowlist: linesOf(allowlist),
        commandTimeoutSecs: values.commandTimeoutSecs,
        memoryLimitMb: values.memoryLimitMb,
        cpuPercent: values.cpuPercent,
      });
      setSaved(next);
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
        <h3 id={ids.title} className={SECTION_TITLE}>Code mode commands</h3>
        <p id={ids.help} className="m-0 text-small text-shodh-text-muted">
          Each conversation picks its mode in the message box: Ask, Accept edits, Plan or Auto. Commands that delete
          files, reset or rewrite git history, or push always ask, whatever the mode.
        </p>
      </div>
      {error && <p role="alert" className="m-0 text-small text-shodh-error">The settings could not be read: {error}</p>}
      <form
        className="flex flex-col gap-3"
        aria-busy={saved === null && !error}
        onSubmit={e => {
          e.preventDefault();
          void save();
        }}
      >
        <div className="flex flex-col gap-1">
          <label htmlFor={ids.allowlist} className={LABEL}>More commands Auto mode runs without asking, one per line</label>
          <textarea
            id={ids.allowlist}
            rows={4}
            value={allowlist}
            onChange={e => setAllowlist(e.target.value)}
            spellCheck={false}
            disabled={saved === null}
            placeholder="make check"
            className={cn(TEXTAREA, 'font-mono text-small')}
          />
          <p className="m-0 text-caption text-shodh-text-muted">
            Auto mode already runs tests, builds, linters, and git and file commands that only read, when each is one plain command. A line here also runs by itself in Auto mode when the command is exactly that line, with nothing added, and has no ; | &amp; &gt; $ or backticks. It still asks while another command is running, and after a change you did not review: tests run code the assistant may have changed.
          </p>
        </div>
        <fieldset
          className="m-0 p-0 border-0 flex flex-col gap-2"
          aria-describedby={ids.limitsHelp}
          disabled={saved === null}
        >
          <legend className={cn(SECTION_TITLE, 'p-0 mb-0.5')}>Command limits</legend>
          <p id={ids.limitsHelp} className="m-0 text-small text-shodh-text-muted">
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
                    className={cn(INPUT, 'w-36 font-mono text-small', invalid && 'border-shodh-error')}
                  />
                  {hint && <span id={hintId} className="text-caption text-shodh-text-muted">{hint}</span>}
                </div>
                {invalid && parsed.ok === false && (
                  <p id={errorId} className="m-0 text-caption text-shodh-error">{parsed.error}</p>
                )}
              </div>
            );
          })}
        </fieldset>
        <div className="flex flex-col gap-0.5">
          <h4 className={SECTION_TITLE}>Command sandbox</h4>
          <p className="m-0 text-small text-shodh-text-muted">{SANDBOX_TEXT}</p>
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
