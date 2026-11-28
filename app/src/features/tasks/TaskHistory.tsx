/**
 * A task's History in its sheet: each recorded change (`task_history`), newest first, with
 * who made it. The words are in `history.ts`.
 */
import { useEffect, useId, useState } from 'react';
import { taskHistory } from './historyApi';
import { historyRows } from './history';
import type { TaskChange, TodoItem } from './types';

/** Rows shown before "Show all". */
const FIRST_ROWS = 5;

export function TaskHistory({ task }: { task: TodoItem }) {
  const headingId = useId();
  const [changes, setChanges] = useState<TaskChange[] | null>(null);
  const [all, setAll] = useState(false);

  // Read again whenever the task changes, so the change just made shows.
  useEffect(() => {
    let current = true;
    taskHistory(task.id)
      .then(found => {
        if (current) setChanges(found);
      })
      .catch(error => {
        console.error('Task history could not be read:', error);
        if (current) setChanges([]);
      });
    return () => {
      current = false;
    };
  }, [task.id, task.updatedAt]);

  if (!changes || changes.length === 0) return null;
  const rows = historyRows(changes, new Date());
  const shown = all ? rows : rows.slice(0, FIRST_ROWS);
  return (
    <section aria-labelledby={headingId} className="px-2.5 flex flex-col gap-1.5">
      <h3 id={headingId} className="m-0 text-caption font-semibold text-shodh-text-muted">History</h3>
      <ul className="m-0 p-0 list-none flex flex-col gap-1">
        {shown.map(row => (
          <li key={row.key} className="text-small text-shodh-text-secondary break-words">
            <span className="font-medium text-shodh-text">{row.who}</span>
            {` · ${row.what}`}
            <span className="text-shodh-text-faint">{` · ${row.when}`}</span>
          </li>
        ))}
      </ul>
      {rows.length > FIRST_ROWS && (
        <button
          type="button"
          onClick={() => setAll(a => !a)}
          className="self-start text-caption text-shodh-accent-text hover:underline"
        >
          {all ? 'Show fewer' : `Show all ${rows.length}`}
        </button>
      )}
    </section>
  );
}
