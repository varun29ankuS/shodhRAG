import { useEffect, useState } from 'react';
import { checkAndMigrate, autoBackupIfNeeded } from '../lib/database';
import { toast } from 'sonner';

export function useDatabaseMigration() {
  const [isReady, setIsReady] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    async function initialize() {
      try {
        const migrationSuccess = await checkAndMigrate();

        if (!migrationSuccess) {
          setError('Database migration failed. Please contact support.');
          toast.error('Database migration failed', { duration: 10000 });
          return;
        }

        await autoBackupIfNeeded();

        setIsReady(true);
      } catch (error) {
        const errorMessage = (error as Error).message;
        setError(errorMessage);
        toast.error(`Database initialization failed: ${errorMessage}`);
      }
    }

    initialize();
  }, []);

  return { isReady, error };
}
