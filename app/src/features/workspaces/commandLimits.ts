/**
 * The limits a Code-mode command runs under, as the workspace's Code settings
 * edit them. Pure, so it is unit-tested directly with Node
 * (`app/tests/commandLimits.test.ts`). The ranges match the backend's, which
 * rejects anything outside them on save.
 */

export type CommandLimitKey = 'commandTimeoutSecs' | 'memoryLimitMb' | 'cpuPercent';

export interface CommandLimitSpec {
  key: CommandLimitKey;
  label: string;
  unit: string;
  min: number;
  max: number;
  defaultValue: number;
}

export const COMMAND_LIMITS: readonly CommandLimitSpec[] = [
  { key: 'commandTimeoutSecs', label: 'Time limit per command', unit: 'seconds', min: 10, max: 3600, defaultValue: 600 },
  { key: 'memoryLimitMb', label: 'Memory limit', unit: 'MB', min: 256, max: 65536, defaultValue: 4096 },
  { key: 'cpuPercent', label: 'CPU cap on Windows', unit: 'percent', min: 10, max: 100, defaultValue: 80 },
];

/** The systems Code-mode commands run on, as `desktopPlatform` names them. */
export type CommandPlatform = 'windows' | 'macos' | 'linux';

/** What Settings says about the command sandbox on `platform`: what the backend enforces there. */
export function commandSandboxText(platform: CommandPlatform): string {
  if (platform === 'macos') {
    return 'Active: commands have no network, can change only the project folder (its .git folder and hooks included) and the conversation\'s scratch folder, and cannot read the rest of your home folder, your keychain or Shodh\'s data. They can still read files outside your home folder, such as other drives.';
  }
  if (platform === 'windows') {
    return "Not available on Windows yet: commands run with your account's access, after your approval, with time and memory limits and without your API keys.";
  }
  return 'Not available: commands do not run on this system.';
}

/** A limit typed by the user, read as a whole number in its range. */
export type ParsedLimit = { ok: true; value: number } | { ok: false; error: string };

export function parseCommandLimit(spec: CommandLimitSpec, text: string): ParsedLimit {
  const trimmed = text.trim();
  const range = `${spec.min.toLocaleString('en-US')} to ${spec.max.toLocaleString('en-US')} ${spec.unit}`;
  if (!/^\d+$/.test(trimmed)) return { ok: false, error: `Enter a whole number from ${range}.` };
  const value = Number(trimmed);
  if (!Number.isSafeInteger(value) || value < spec.min || value > spec.max) {
    return { ok: false, error: `Must be from ${range}.` };
  }
  return { ok: true, value };
}

/** A limit in friendlier words, for the hint beside its field: "10 min", "4 GB"; null when the number says it. */
export function commandLimitHint(key: CommandLimitKey, value: number): string | null {
  if (key === 'commandTimeoutSecs') {
    if (value < 60) return null;
    const minutes = value / 60;
    if (value % 3600 === 0) return value === 3600 ? '1 hour' : `${value / 3600} hours`;
    return `${Number.isInteger(minutes) ? minutes : minutes.toFixed(1)} min`;
  }
  if (key === 'memoryLimitMb') {
    if (value < 1024) return null;
    const gb = value / 1024;
    return `${Number.isInteger(gb) ? gb : gb.toFixed(1)} GB`;
  }
  return null;
}
