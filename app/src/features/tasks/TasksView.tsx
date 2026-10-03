import React, { useEffect, useState } from 'react';
import { CalendarDays, List } from 'lucide-react';
import CalendarTodoPanel from '../../components/CalendarTodoPanel';
import { cn } from '../../lib/utils';
import { useChatSession } from '../ask/ChatSessionContext';
import TasksCalendar from './TasksCalendar';
import { TasksStoreProvider, useTasksStore } from './TasksStore';
import TaskDetailSheet from './TaskDetailSheet';
import EventDetailSheet from './EventDetailSheet';

export type TasksMode = 'list' | 'calendar';

const STORAGE_KEY = 'shodh.tasksMode';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

function readMode(): TasksMode {
  try {
    return window.localStorage.getItem(STORAGE_KEY) === 'calendar' ? 'calendar' : 'list';
  } catch {
    return 'list';
  }
}

function storeMode(mode: TasksMode) {
  try {
    window.localStorage.setItem(STORAGE_KEY, mode);
  } catch {
    // Storage unavailable; the choice lasts for this session only.
  }
}

/**
 * Highest agent navigation already applied. Module scope, because the
 * navigation that opens Tasks usually arrives before this view mounts.
 */
let appliedNavSeq = 0;

const MODES: { id: TasksMode; label: string; icon: React.ElementType }[] = [
  { id: 'list', label: 'List', icon: List },
  { id: 'calendar', label: 'Calendar', icon: CalendarDays },
];

/** Detail sheets for whichever task or event the store has open. */
function DetailSheets() {
  const { detailTask, detailEvent, closeDetail } = useTasksStore();
  return (
    <>
      <TaskDetailSheet task={detailTask} onClose={closeDetail} />
      <EventDetailSheet event={detailEvent} onClose={closeDetail} />
    </>
  );
}

/**
 * Tasks: list first (the task panel), with a calendar view of the same
 * tasks and events. The choice is remembered per viewer. Both layouts share
 * one store, mounted above the layout switch so edits in flight and pending
 * undo windows survive switching.
 */
export default function TasksView() {
  return (
    <TasksStoreProvider>
      <TasksLayouts />
      <DetailSheets />
    </TasksStoreProvider>
  );
}

function TasksLayouts() {
  const [mode, setMode] = useState<TasksMode>(readMode);
  const { navigation } = useChatSession();

  // The agent opened a specific task: show the list, where it is focused.
  useEffect(() => {
    if (!navigation || navigation.seq <= appliedNavSeq) return;
    appliedNavSeq = navigation.seq;
    if (navigation.view === 'tasks' && navigation.focus) setMode('list');
  }, [navigation]);

  const choose = (next: TasksMode) => {
    setMode(next);
    storeMode(next);
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
    e.preventDefault();
    const index = MODES.findIndex(m => m.id === mode);
    const next = MODES[(index + (e.key === 'ArrowRight' ? 1 : -1) + MODES.length) % MODES.length];
    choose(next.id);
    e.currentTarget.querySelector<HTMLButtonElement>(`[data-mode="${next.id}"]`)?.focus();
  };

  return (
    <div className="h-full flex flex-col min-h-0">
      <div className="px-6 pt-4 shrink-0 flex items-center">
        <div
          role="radiogroup"
          aria-label="Tasks layout"
          onKeyDown={handleKeyDown}
          className="inline-flex p-0.5 rounded-lg bg-shodh-raised border border-shodh-border"
        >
          {MODES.map(m => {
            const Icon = m.icon;
            const checked = m.id === mode;
            return (
              <button
                key={m.id}
                type="button"
                role="radio"
                aria-checked={checked}
                tabIndex={checked ? 0 : -1}
                data-mode={m.id}
                onClick={() => choose(m.id)}
                className={cn(
                  'h-7 px-2.5 inline-flex items-center gap-1.5 rounded-md text-[12.5px] transition-colors duration-micro',
                  checked
                    ? 'bg-shodh-surface text-shodh-text font-semibold shadow-sm'
                    : 'text-shodh-text-muted hover:text-shodh-text',
                  FOCUS_RING,
                )}
              >
                <Icon className="w-3.5 h-3.5" aria-hidden="true" />
                {m.label}
              </button>
            );
          })}
        </div>
      </div>
      <div key={mode} className="shell-view-enter flex-1 min-h-0">
        {mode === 'list' ? <CalendarTodoPanel /> : (
          <div className="h-full flex flex-col min-h-0">
            <h1 className="px-6 pt-3 text-lg font-bold shrink-0">Tasks</h1>
            <div className="flex-1 min-h-0">
              <TasksCalendar />
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
