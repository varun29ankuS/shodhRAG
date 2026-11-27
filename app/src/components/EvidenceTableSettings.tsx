import React, { useEffect, useId, useState } from 'react';
import { notify } from '../lib/notify';
import { onAppSettingsChanged } from '../lib/appSettings';
import { cn } from '../lib/utils';
import { SwitchRow } from './PrivacySettings';
import { tableError, tablesApi } from '../features/tables/api';
import type { TablesSettings } from '../features/tables/types';
import { localTime, nextUtcMidnight } from '../features/tables/format';
import { tr } from '../i18n/tr';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-surface';

/**
 * Settings → Tables: the day's cap of model calls evidence tables may make (every table,
 * fills and refills alike), and the diagnostics switch that allows a quality check of one
 * paper with its model calls kept. Only the user changes these.
 */
export default function EvidenceTableSettings() {
  const [settings, setSettings] = useState<TablesSettings | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [capInput, setCapInput] = useState('');
  const [saving, setSaving] = useState(false);
  const capId = useId();
  const capHelpId = useId();

  useEffect(() => {
    let cancelled = false;
    const load = () => {
      tablesApi
        .settings()
        .then(next => {
          if (cancelled) return;
          setSettings(next);
          setCapInput(String(next.dailyCallCap));
          setLoadError(null);
        })
        .catch(err => { if (!cancelled) setLoadError(tableError(err).message); });
    };
    load();
    const unsubscribe = onAppSettingsChanged(load);
    return () => {
      cancelled = true;
      unsubscribe();
    };
  }, []);

  if (loadError && !settings) {
    return <p role="alert" className="m-0 text-small text-shodh-error">{tr('settings.evidenceTableSettings.theTablesSettingsCouldNot')}{loadError}</p>;
  }
  if (!settings) {
    return <p className="m-0 text-small text-shodh-text-muted">{tr('settings.evidenceTableSettings.loading')}</p>;
  }

  const cap = Number(capInput);
  const capValid = Number.isInteger(cap) && cap >= settings.minDailyCallCap && cap <= settings.maxDailyCallCap;
  const capChanged = capValid && cap !== settings.dailyCallCap;

  const save = async (dailyCallCap: number, diagnostics?: boolean) => {
    setSaving(true);
    try {
      const next = await tablesApi.setSettings(dailyCallCap, diagnostics);
      setSettings(next);
      setCapInput(String(next.dailyCallCap));
    } catch (err) {
      notify.error('The tables setting was not saved', { description: tableError(err).message });
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="flex flex-col gap-5">
      <form
        className="flex items-start justify-between gap-6"
        onSubmit={e => {
          e.preventDefault();
          if (capChanged) void save(cap);
        }}
      >
        <div className="min-w-0">
          <label htmlFor={capId} className="m-0 text-body font-semibold text-shodh-text">{tr('settings.evidenceTableSettings.modelCallsTablesMayMake')}</label>
          <p id={capHelpId} className="m-0 mt-0.5 text-small text-shodh-text-muted">
            {tr('settings.evidenceTableSettings.fillingAPaperTakesOne')}{localTime(nextUtcMidnight(Date.now()), Date.now())}{tr('settings.evidenceTableSettings.midnightUtcInYourTime')}{settings.minDailyCallCap}{tr('settings.evidenceTableSettings.to')}{settings.maxDailyCallCap}.
          </p>
          {!capValid && capInput.trim() !== '' && (
            <p role="alert" className="m-0 mt-1 text-small text-shodh-error">
              {tr('settings.evidenceTableSettings.enterAWholeNumberFrom')}{settings.minDailyCallCap}{tr('settings.evidenceTableSettings.to')}{settings.maxDailyCallCap}.
            </p>
          )}
        </div>
        <div className="shrink-0 flex items-center gap-2">
          <input
            id={capId}
            type="number"
            inputMode="numeric"
            min={settings.minDailyCallCap}
            max={settings.maxDailyCallCap}
            step={1}
            value={capInput}
            disabled={saving}
            aria-describedby={capHelpId}
            aria-invalid={!capValid}
            onChange={e => setCapInput(e.target.value)}
            className={cn('w-20 h-8 rounded-lg border border-shodh-border bg-shodh-ground px-2 text-small text-shodh-text tabular-nums disabled:opacity-50', FOCUS_RING)}
          />
          <button
            type="submit"
            disabled={!capChanged || saving}
            className={cn('h-8 px-3 rounded-lg border border-shodh-border text-small text-shodh-text-secondary hover:bg-shodh-raised disabled:opacity-50 disabled:cursor-not-allowed', FOCUS_RING)}
          >
            {tr('settings.evidenceTableSettings.save')}
          </button>
        </div>
      </form>
      <SwitchRow
        label="Table diagnostics"
        description={'Keeps each model call for export, on this computer.'}
        checked={settings.diagnostics}
        disabled={saving}
        onChange={on => void save(settings.dailyCallCap, on)}
      />
    </div>
  );
}
