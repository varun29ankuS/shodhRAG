/**
 * The answer engine's shared setup state, apart from the provider that runs it
 * (`RuntimeSetupContext.tsx`). Components that show it (the runtime card in a transcript,
 * the setup card) read it through `useRuntimeSetup`, so they render under any provider of
 * this value: the main window's, which checks and installs the engine, and the floating
 * chat's (`features/dock`), which shows main's state from its snapshot and sends the
 * buttons to main.
 */
import { createContext, useContext } from 'react';
import type { RuntimeInstall, RuntimeProgress, RuntimeStatus } from '../agent/agentApi';

/** What the setup shows, as plain data (the floating chat receives it in its snapshot). */
export interface RuntimeSetupState {
  /** Latest status, or null until the first check finishes. */
  status: RuntimeStatus | null;
  statusError: string | null;
  /**
   * Whether the answer engine is on this computer (null: not known yet). A start
   * that failed because it is missing sets it false (`setInstalled`).
   */
  installed: boolean | null;
  installing: boolean;
  progress: RuntimeProgress | null;
  installError: string | null;
  /** The last install could not connect. */
  installOffline: boolean;
  /** Result of an install that finished in this session. */
  justInstalled: RuntimeInstall | null;
  /** When that install finished (ms), so a card shown after it is not covered by it. */
  installedAt: number | null;
}

export interface RuntimeSetupValue extends RuntimeSetupState {
  dismissInstalled: () => void;
  /** Download and verify it; resolves whether it is installed afterwards. */
  install: () => Promise<boolean>;
  setInstalled: (installed: boolean) => void;
  refresh: () => Promise<void>;
}

export const RuntimeSetupContext = createContext<RuntimeSetupValue | null>(null);

export function useRuntimeSetup(): RuntimeSetupValue {
  const context = useContext(RuntimeSetupContext);
  if (!context) throw new Error('useRuntimeSetup must be used within a RuntimeSetupProvider');
  return context;
}

/** The plain state of `value`, as the floating chat's snapshot carries it. */
export function runtimeSetupState(value: RuntimeSetupState): RuntimeSetupState {
  return {
    status: value.status,
    statusError: value.statusError,
    installed: value.installed,
    installing: value.installing,
    progress: value.progress,
    installError: value.installError,
    installOffline: value.installOffline,
    justInstalled: value.justInstalled,
    installedAt: value.installedAt,
  };
}
