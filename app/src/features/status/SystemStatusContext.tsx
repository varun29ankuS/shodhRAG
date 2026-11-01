/**
 * One System status for the whole window (the status strip, its panel, the
 * activity tray and the Library's CPU notice read the same state), and the
 * loud warning when search falls back to the CPU: a toast the first time a
 * graphics card failed or ran slower than the processor in this session.
 */
import { createContext, useContext, useEffect, useRef } from 'react';
import type React from 'react';
import { notify } from '../../lib/notify';
import { cpuNotice } from './systemStatus';
import { useSystemStatus } from './useSystemStatus';
import type { SystemStatusState } from './useSystemStatus';

const SystemStatusContext = createContext<SystemStatusState | null>(null);

export function SystemStatusProvider({ children }: { children: React.ReactNode }) {
  const state = useSystemStatus();
  const warned = useRef(false);
  const report = state.status?.search ?? null;
  const fellBack =
    report?.accelerator === 'cpu' &&
    (report.cpuReason?.kind === 'failed' || report.cpuReason?.kind === 'slowerThanCpu');

  useEffect(() => {
    if (!fellBack || warned.current) return;
    const notice = cpuNotice(state.status);
    if (!notice) return;
    warned.current = true;
    notify.warning(notice.title, { description: `${notice.reason} ${notice.advice}` });
  }, [fellBack, state.status]);

  return <SystemStatusContext.Provider value={state}>{children}</SystemStatusContext.Provider>;
}

/** The window's System status; `null` outside the provider. */
export function useSystemStatusContext(): SystemStatusState | null {
  return useContext(SystemStatusContext);
}
