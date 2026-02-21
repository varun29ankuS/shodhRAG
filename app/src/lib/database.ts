import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { captureError, addBreadcrumb } from './errorReporting';
import { APP_VERSION } from './version';

export const CURRENT_DB_VERSION = 1;
const DB_VERSION_KEY = 'db_version';
const LAST_BACKUP_KEY = 'last_backup_date';

export interface DatabaseVersion {
  version: number;
  appVersion: string;
  migratedAt: string;
}

export interface BackupMetadata {
  version: number;
  appVersion: string;
  createdAt: string;
  spacesCount: number;
  size?: number;
}

export interface MigrationResult {
  success: boolean;
  fromVersion: number;
  toVersion: number;
  error?: string;
  backupPath?: string;
}

export function getCurrentDbVersion(): number {
  const stored = localStorage.getItem(DB_VERSION_KEY);
  return stored ? parseInt(stored, 10) : 0;
}

export function setDbVersion(version: number): void {
  const versionData: DatabaseVersion = {
    version,
    appVersion: APP_VERSION,
    migratedAt: new Date().toISOString(),
  };
  localStorage.setItem(DB_VERSION_KEY, JSON.stringify(versionData));
  addBreadcrumb('Database version updated', 'database', { version, appVersion: APP_VERSION });
}

export function getDbVersionInfo(): DatabaseVersion | null {
  const stored = localStorage.getItem(DB_VERSION_KEY);
  if (!stored) return null;

  try {
    return JSON.parse(stored);
  } catch {
    return null;
  }
}

export async function needsMigration(): Promise<boolean> {
  const current = getCurrentDbVersion();
  return current < CURRENT_DB_VERSION;
}

export async function createBackup(reason: string = 'manual'): Promise<string | null> {
  try {
    addBreadcrumb('Creating backup', 'database', { reason });

    const spaces = await invoke<any[]>('get_spaces');
    const timestamp = new Date().toISOString().replace(/[:.]/g, '-');
    const backupName = `backup_v${getCurrentDbVersion()}_${timestamp}.json`;

    const metadata: BackupMetadata = {
      version: getCurrentDbVersion(),
      appVersion: APP_VERSION,
      createdAt: new Date().toISOString(),
      spacesCount: spaces.length,
    };

    const backupData = {
      metadata,
      spaces,
      dbVersion: getDbVersionInfo(),
    };

    const backupPath = await invoke<string>('save_backup_file', {
      fileName: backupName,
      data: JSON.stringify(backupData, null, 2),
    });

    localStorage.setItem(LAST_BACKUP_KEY, new Date().toISOString());
    addBreadcrumb('Backup created', 'database', { backupPath, spacesCount: spaces.length });

    return backupPath;
  } catch (error) {
    captureError(error as Error, { context: 'create_backup', reason });
    toast.error(`Failed to create backup: ${(error as Error).message}`);
    return null;
  }
}

export async function restoreFromBackup(backupPath: string): Promise<boolean> {
  try {
    addBreadcrumb('Restoring from backup', 'database', { backupPath });

    const backupData = await invoke<string>('read_backup_file', { backupPath });
    const parsed = JSON.parse(backupData);

    if (!parsed.metadata || !parsed.spaces) {
      throw new Error('Invalid backup format');
    }

    const currentBackup = await createBackup('pre-restore');
    if (!currentBackup) {
      throw new Error('Failed to create safety backup before restore');
    }

    for (const space of parsed.spaces) {
      try {
        await invoke('restore_space_from_backup', { spaceData: space });
      } catch (error) {
        console.error(`Failed to restore space ${space.id}:`, error);
      }
    }

    if (parsed.dbVersion) {
      setDbVersion(parsed.dbVersion.version);
    }

    addBreadcrumb('Backup restored', 'database', {
      backupPath,
      spacesRestored: parsed.spaces.length
    });

    toast.success(`Restored ${parsed.spaces.length} workspaces from backup`);
    return true;
  } catch (error) {
    captureError(error as Error, { context: 'restore_backup', backupPath });
    toast.error(`Failed to restore backup: ${(error as Error).message}`);
    return false;
  }
}

export async function runMigrations(): Promise<MigrationResult> {
  const fromVersion = getCurrentDbVersion();
  const toVersion = CURRENT_DB_VERSION;

  if (fromVersion === toVersion) {
    return { success: true, fromVersion, toVersion };
  }

  addBreadcrumb('Starting migration', 'database', { fromVersion, toVersion });

  try {
    const backupPath = await createBackup('pre-migration');
    if (!backupPath) {
      throw new Error('Failed to create pre-migration backup');
    }

    for (let version = fromVersion; version < toVersion; version++) {
      const nextVersion = version + 1;
      addBreadcrumb(`Migrating to version ${nextVersion}`, 'database', { version: nextVersion });

      await runMigration(version, nextVersion);
    }

    setDbVersion(toVersion);

    addBreadcrumb('Migration completed', 'database', { fromVersion, toVersion });
    toast.success(`Database upgraded from v${fromVersion} to v${toVersion}`);

    return {
      success: true,
      fromVersion,
      toVersion,
      backupPath,
    };
  } catch (error) {
    captureError(error as Error, { context: 'run_migrations', fromVersion, toVersion });

    return {
      success: false,
      fromVersion,
      toVersion,
      error: (error as Error).message,
    };
  }
}

async function runMigration(from: number, to: number): Promise<void> {
  switch (to) {
    case 1:
      await migrateToV1();
      break;
    default:
      throw new Error(`Unknown migration target: ${to}`);
  }
}

async function migrateToV1(): Promise<void> {
  addBreadcrumb('Running migration to v1', 'database');

  try {
    const spaces = await invoke<any[]>('get_spaces');

    for (const space of spaces) {
      if (!space.metadata) {
        await invoke('update_space_metadata', {
          spaceId: space.id,
          metadata: {
            created_at: new Date().toISOString(),
            indexed_at: space.indexed_at || new Date().toISOString(),
            version: 1,
          },
        });
      }
    }

    addBreadcrumb('Migration to v1 completed', 'database', { spacesUpdated: spaces.length });
  } catch (error) {
    throw new Error(`Migration to v1 failed: ${(error as Error).message}`);
  }
}

export async function checkAndMigrate(): Promise<boolean> {
  try {
    if (await needsMigration()) {
      const fromVersion = getCurrentDbVersion();

      toast.info('Database update required. Creating backup...', { duration: 3000 });

      const result = await runMigrations();

      if (!result.success) {
        toast.error('Database migration failed. Please contact support.', { duration: 10000 });
        captureError(new Error('Migration failed'), {
          context: 'check_and_migrate',
          result
        });
        return false;
      }

      if (fromVersion === 0) {
        setDbVersion(CURRENT_DB_VERSION);
      }

      return true;
    }

    if (getCurrentDbVersion() === 0) {
      setDbVersion(CURRENT_DB_VERSION);
    }

    return true;
  } catch (error) {
    captureError(error as Error, { context: 'check_and_migrate' });
    return false;
  }
}

export function getLastBackupDate(): Date | null {
  const stored = localStorage.getItem(LAST_BACKUP_KEY);
  return stored ? new Date(stored) : null;
}

export function shouldCreateBackup(): boolean {
  const lastBackup = getLastBackupDate();
  if (!lastBackup) return true;

  const daysSinceBackup = (Date.now() - lastBackup.getTime()) / (1000 * 60 * 60 * 24);
  return daysSinceBackup >= 7;
}

export async function autoBackupIfNeeded(): Promise<void> {
  if (shouldCreateBackup()) {
    addBreadcrumb('Auto-backup triggered', 'database');
    const backupPath = await createBackup('auto');
    if (backupPath) {
      toast.success('Automatic backup created', { duration: 3000 });
    }
  }
}
