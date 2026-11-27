import React, { useEffect, useId, useState } from 'react';
import { cn } from '../lib/utils';
import { notify } from '../lib/notify';
import { getAppSettings, onAppSettingsChanged, setCodeMode, setDefaultAgentMode, setPolicy } from '../lib/appSettings';
import { AGENT_MODES, DEFAULTABLE_MODES, MODE_FLOOR } from '../features/agent/agentModes';
import type { AgentMode } from '../features/agent/agentModes';
import { MODE_ICONS } from '../features/agent/ModePicker';
import type { Policy } from '../lib/appSettings';
import { checkForUpdate, getUpdateStatus, setUpdateChecks } from '../lib/updates';
import type { UpdateStatus } from '../lib/updates';
import { tr } from '../i18n/tr';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

const BUTTON = cn(
  'h-8 px-3 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border bg-shodh-surface text-small text-shodh-text',
  'hover:bg-shodh-raised disabled:opacity-50 disabled:cursor-not-allowed transition-colors duration-micro',
  FOCUS_RING,
);

interface SwitchRowProps {
  label: string;
  description: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (checked: boolean) => void;
}

export function SwitchRow({ label, description, checked, disabled = false, onChange }: SwitchRowProps) {
  const labelId = useId();
  const descriptionId = useId();
  return (
    <div className="flex items-start justify-between gap-6">
      <div className="min-w-0">
        <p id={labelId} className="m-0 text-body font-semibold text-shodh-text">{label}</p>
        <p id={descriptionId} className="m-0 mt-0.5 text-small text-shodh-text-muted">{description}</p>
      </div>
      <button
        type="button"
        role="switch"
        aria-checked={checked}
        aria-labelledby={labelId}
        aria-describedby={descriptionId}
        disabled={disabled}
        onClick={() => onChange(!checked)}
        className={cn(
          'target-24 relative shrink-0 mt-0.5 w-9 h-5 rounded-full transition-colors duration-micro disabled:opacity-50 disabled:cursor-not-allowed',
          checked ? 'bg-shodh-accent' : 'bg-shodh-raised-2',
          FOCUS_RING,
        )}
      >
        <span
          aria-hidden="true"
          className={cn(
            'absolute top-0.5 w-4 h-4 rounded-full bg-white shadow transition-[left] duration-micro',
            checked ? 'start-[18px]' : 'start-0.5',
          )}
        />
      </button>
    </div>
  );
}

/**
 * Settings → Privacy: where the user's data may go. Only the user can
 * change these; the assistant can read them but has no tool to change them.
 */
export default function PrivacySettings() {
  const [policy, setPolicyState] = useState<Policy | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let cancelled = false;
    getAppSettings()
      .then(settings => {
        if (!cancelled && settings) setPolicyState(settings.policy);
      })
      .catch(err => notify.error('Privacy settings could not be loaded', { description: String(err) }));
    const unsubscribe = onAppSettingsChanged(settings => setPolicyState(settings.policy));
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);

  const change = async (next: Policy) => {
    setSaving(true);
    try {
      const saved = await setPolicy(next);
      if (saved) setPolicyState(saved.policy);
    } catch (err) {
      notify.error('The privacy setting was not saved', { description: String(err) });
    } finally {
      setSaving(false);
    }
  };

  if (!policy) {
    return <p className="m-0 text-small text-shodh-text-muted">{tr('settings.privacySettings.loading')}</p>;
  }

  return (
    <div className="flex flex-col gap-5">
      <SwitchRow
        label="Local-only mode"
        description={'Nothing leaves this computer; local models only.'}
        checked={policy.localOnly}
        disabled={saving}
        onChange={localOnly => void change({ ...policy, localOnly })}
      />
      <SwitchRow
        label="Let the assistant use the web"
        description={'Web search, pages and papers. Every request is logged.'}
        checked={policy.webAccess && !policy.localOnly}
        disabled={saving || policy.localOnly}
        onChange={webAccess => void change({ ...policy, webAccess })}
      />
      <DefaultModeSettings />
      <CodeModeSettings />
      <UpdateSettings />
    </div>
  );
}

/**
 * The mode new conversations answer in. Each conversation keeps the mode picked for it
 * in the message box; one without its own follows this. Only the user changes it.
 */
function DefaultModeSettings() {
  const [mode, setMode] = useState<AgentMode | null>(null);
  const [saving, setSaving] = useState(false);
  const titleId = useId();
  const helpId = useId();

  useEffect(() => {
    let cancelled = false;
    getAppSettings()
      .then(settings => {
        if (!cancelled && settings) setMode(settings.agent.defaultMode);
      })
      .catch(err => notify.error('The default mode could not be loaded', { description: String(err) }));
    const unsubscribe = onAppSettingsChanged(settings => setMode(settings.agent.defaultMode));
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);

  const save = async (next: AgentMode) => {
    setSaving(true);
    try {
      const saved = await setDefaultAgentMode(next);
      if (saved) setMode(saved.agent.defaultMode);
    } catch (err) {
      notify.error('The default mode was not saved', { description: String(err) });
    } finally {
      setSaving(false);
    }
  };

  if (mode === null) return null;
  return (
    <div role="radiogroup" aria-labelledby={titleId} aria-describedby={helpId} className="flex flex-col gap-2 pt-5 border-t border-shodh-border">
      <div>
        <p id={titleId} className="m-0 text-body font-semibold text-shodh-text">{tr('settings.privacySettings.defaultMode')}</p>
        <p id={helpId} className="m-0 mt-0.5 text-small text-shodh-text-muted">
          {tr('settings.privacySettings.theModeANewConversation')}{MODE_FLOOR}
        </p>
      </div>
      <div className="flex flex-col gap-1">
        {AGENT_MODES.filter(info => DEFAULTABLE_MODES.includes(info.id)).map(info => {
          const Icon = MODE_ICONS[info.id];
          const selected = info.id === mode;
          return (
            <label
              key={info.id}
              className={cn(
                'flex items-start gap-2.5 px-3 py-2 rounded-lg border cursor-pointer transition-colors duration-micro',
                selected ? 'border-shodh-border-strong bg-shodh-raised' : 'border-transparent hover:bg-shodh-raised',
              )}
            >
              <input
                type="radio"
                name={`${titleId}-mode`}
                value={info.id}
                checked={selected}
                disabled={saving}
                onChange={() => void save(info.id)}
                className={cn('mt-0.5 accent-shodh-accent', FOCUS_RING)}
              />
              <span className="flex flex-col">
                <span className="inline-flex items-center gap-1.5 text-small font-medium text-shodh-text">
                  <Icon className="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
                  {info.label}
                </span>
                <span className="text-caption text-shodh-text-muted">{info.hint}</span>
              </span>
            </label>
          );
        })}
      </div>
    </div>
  );
}

/**
 * Code mode: off on a new install. Turning it on first says plainly what it
 * lets answers do, and asks once more; turning it off takes effect at once.
 */
function CodeModeSettings() {
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [saving, setSaving] = useState(false);
  const explainId = useId();

  useEffect(() => {
    let cancelled = false;
    getAppSettings()
      .then(settings => {
        if (!cancelled && settings) setEnabled(settings.code.enabled);
      })
      .catch(err => notify.error('The Code mode setting could not be loaded', { description: String(err) }));
    const unsubscribe = onAppSettingsChanged(settings => setEnabled(settings.code.enabled));
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);

  const save = async (next: boolean) => {
    setSaving(true);
    try {
      const saved = await setCodeMode(next);
      if (saved) setEnabled(saved.code.enabled);
      setConfirming(false);
    } catch (err) {
      notify.error('The Code mode setting was not saved', { description: String(err) });
    } finally {
      setSaving(false);
    }
  };

  if (enabled === null) return null;
  return (
    <div className="flex flex-col gap-3 pt-5 border-t border-shodh-border">
      <SwitchRow
        label="Code mode"
        description={'Lets a workspace chat edit files and run commands.'}
        checked={enabled}
        disabled={saving || confirming}
        onChange={next => (next ? setConfirming(true) : void save(false))}
      />
      {confirming && (
        <div role="group" aria-labelledby={explainId} className="flex flex-col gap-2 p-3 rounded-xl border border-shodh-border bg-shodh-raised">
          <p id={explainId} className="m-0 text-small font-semibold text-shodh-text">{tr('settings.privacySettings.turnOnCodeMode')}</p>
          <p className="m-0 text-small text-shodh-text-secondary">
            {tr('settings.privacySettings.inCodeModeTheAssistant')}
          </p>
          <div className="flex flex-wrap items-center gap-2">
            <button type="button" className={BUTTON} onClick={() => void save(true)} disabled={saving}>
              {tr('settings.privacySettings.turnOnCodeMode2')}
            </button>
            <button type="button" className={BUTTON} onClick={() => setConfirming(false)} disabled={saving}>
              {tr('settings.privacySettings.cancel')}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

/**
 * Whether Shodh asks the release server for a newer version of itself. The
 * check sends nothing about the user or their documents; an update found is
 * shown with an Install button and installed only when the user chooses.
 */
function UpdateSettings() {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    const refresh = () =>
      getUpdateStatus()
        .then(next => {
          if (!cancelled) setStatus(next);
        })
        .catch(err => notify.error('Update settings could not be loaded', { description: String(err) }));
    void refresh();
    // Local-only mode changes whether checks may run.
    const unsubscribe = onAppSettingsChanged(() => void refresh());
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);

  if (!status) return null;

  const toggle = async (enabled: boolean) => {
    setBusy(true);
    try {
      setStatus(await setUpdateChecks(enabled));
    } catch (err) {
      notify.error('The update setting was not saved', { description: String(err) });
    } finally {
      setBusy(false);
    }
  };

  const checkNow = async () => {
    setBusy(true);
    setResult(null);
    try {
      const check = await checkForUpdate();
      // The status holds the newest check (this one, or a daily one that ran
      // right after it), so the line below never disagrees with the backend.
      try {
        setStatus(await getUpdateStatus());
      } catch {
        setResult(check.message);
      }
    } catch (err) {
      setResult(String(err));
    } finally {
      setBusy(false);
    }
  };

  const blocked = status.blockedReason !== null;
  return (
    <div className="flex flex-col gap-3 pt-5 border-t border-shodh-border">
      <SwitchRow
        label="Check for updates"
        description={'Checks GitHub daily; installs only when you choose. Sends nothing about you.'}
        checked={status.checkAutomatically && !blocked}
        disabled={busy || blocked}
        onChange={enabled => void toggle(enabled)}
      />
      <div className="flex flex-wrap items-center gap-3">
        <button type="button" className={BUTTON} onClick={() => void checkNow()} disabled={busy || blocked}>
          {tr('settings.privacySettings.checkNow')}
        </button>
        <span className="text-small text-shodh-text-muted">{tr('settings.privacySettings.version')}{status.currentVersion}</span>
      </div>
      {blocked ? (
        <p className="m-0 text-small text-shodh-text-muted">{status.blockedReason}</p>
      ) : result ? (
        <p className="m-0 text-small text-shodh-text-muted" role="status">{result}</p>
      ) : status.lastCheck ? (
        <p className="m-0 text-small text-shodh-text-muted" role="status">
          {status.lastCheck.message}
        </p>
      ) : null}
    </div>
  );
}
