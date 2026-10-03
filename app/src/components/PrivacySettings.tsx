import React, { useEffect, useId, useState } from 'react';
import { cn } from '../lib/utils';
import { notify } from '../lib/notify';
import { getAppSettings, onAppSettingsChanged, setPolicy } from '../lib/appSettings';
import type { Policy } from '../lib/appSettings';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

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
    </div>
  );
}
