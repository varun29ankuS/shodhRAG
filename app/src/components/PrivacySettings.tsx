import React, { useEffect, useId, useState } from 'react';
import { cn } from '../lib/utils';
import { notify } from '../lib/notify';
import { getAppSettings, onAppSettingsChanged, setPolicy } from '../lib/appSettings';
import type { Policy } from '../lib/appSettings';
import { checkForUpdate, getUpdateStatus, setUpdateChecks } from '../lib/updates';
import type { UpdateStatus } from '../lib/updates';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

const BUTTON = cn(
  'h-8 px-3 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border bg-shodh-surface text-[12.5px] text-shodh-text',
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
        <p id={labelId} className="m-0 text-[13.5px] font-semibold text-shodh-text">{label}</p>
        <p id={descriptionId} className="m-0 mt-0.5 text-[12.5px] text-shodh-text-muted">{description}</p>
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
          'relative shrink-0 mt-0.5 w-9 h-5 rounded-full transition-colors duration-micro disabled:opacity-50 disabled:cursor-not-allowed',
          checked ? 'bg-shodh-accent' : 'bg-shodh-raised-2',
          FOCUS_RING,
        )}
      >
        <span
          aria-hidden="true"
          className={cn(
            'absolute top-0.5 w-4 h-4 rounded-full bg-white shadow transition-[left] duration-micro',
            checked ? 'left-[18px]' : 'left-0.5',
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
    return <p className="m-0 text-[13px] text-shodh-text-muted">Loading…</p>;
  }

  return (
    <div className="flex flex-col gap-5">
      <SwitchRow
        label="Local-only mode"
        description="Nothing leaves this computer: the assistant cannot use the web, and only a local model (Ollama) may answer. Applies to the assistant in Ask."
        checked={policy.localOnly}
        disabled={saving}
        onChange={localOnly => void change({ ...policy, localOnly })}
      />
      <SwitchRow
        label="Let the assistant use the web"
        description="Allows web search, reading web pages, searching papers and downloading open files into your source folders. Every request is recorded in Usage & Audit."
        checked={policy.webAccess && !policy.localOnly}
        disabled={saving || policy.localOnly}
        onChange={webAccess => void change({ ...policy, webAccess })}
      />
      <UpdateSettings />
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
        description="A few minutes after start and then once a day, Shodh asks GitHub whether a newer version exists. Nothing about you or your documents is sent, and nothing is installed until you choose to. Never in Local-only mode."
        checked={status.checkAutomatically && !blocked}
        disabled={busy || blocked}
        onChange={enabled => void toggle(enabled)}
      />
      <div className="flex flex-wrap items-center gap-3">
        <button type="button" className={BUTTON} onClick={() => void checkNow()} disabled={busy || blocked}>
          Check now
        </button>
        <span className="text-[12.5px] text-shodh-text-muted">Version {status.currentVersion}</span>
      </div>
      {blocked ? (
        <p className="m-0 text-[12.5px] text-shodh-text-muted">{status.blockedReason}</p>
      ) : result ? (
        <p className="m-0 text-[12.5px] text-shodh-text-muted" role="status">{result}</p>
      ) : status.lastCheck ? (
        <p className="m-0 text-[12.5px] text-shodh-text-muted" role="status">
          {status.lastCheck.message}
        </p>
      ) : null}
    </div>
  );
}
