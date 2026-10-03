import React, { useId, useRef, useState } from 'react';
import { Bot, Check, FileText, Plus, Trash2, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import DetailSheet from './DetailSheet';
import { ChipInput, DateTimeField, FIELD_LABEL, FOCUS_RING, INLINE_INPUT, InlineText, Segmented } from './fields';
import { useTasksStore } from './TasksStore';
import { isTempSubtaskId } from './taskStore';
import { PRIORITIES, PRIORITY_LABELS, STATUSES, STATUS_LABELS } from './types';
import type { TodoItem } from './types';

const PRIORITY_SELECTED: Record<string, string> = {
  high: 'text-shodh-error',
  medium: 'text-shodh-warning',
  low: 'text-shodh-success',
};

function formatStamp(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime())
    ? iso
    : d.toLocaleString(undefined, { month: 'short', day: 'numeric', year: 'numeric', hour: 'numeric', minute: '2-digit' });
}

/** Where a task came from, when not typed in by the user. */
export function Provenance({ source, sourceRef }: { source: string; sourceRef?: string | null }) {
  if (source !== 'agent' && source !== 'document' && !sourceRef) return null;
  const Icon = source === 'document' ? FileText : Bot;
  const label = source === 'agent' ? 'Created by agent' : source === 'document' ? 'Created from a document' : 'Linked source';
  return (
    <div className="mx-2.5 flex items-start gap-2 rounded-lg border border-shodh-border bg-shodh-raised px-3 py-2">
      <Icon className="w-3.5 h-3.5 mt-0.5 shrink-0 text-shodh-accent-text" aria-hidden="true" />
      <div className="min-w-0 flex flex-col">
        <span className="text-[12.5px] font-medium text-shodh-text">{label}</span>
        {sourceRef && (
          <span className="text-[11.5px] text-shodh-text-muted break-all font-mono" title={sourceRef}>
            {sourceRef}
          </span>
        )}
      </div>
    </div>
  );
}

function SubtaskList({ task }: { task: TodoItem }) {
  const { addSubtask, toggleSubtask, deleteSubtask } = useTasksStore();
  const [draft, setDraft] = useState('');
  const headingId = useId();
  const inputId = useId();
  const inputRef = useRef<HTMLInputElement>(null);
  const done = task.subtasks.filter(s => s.completed).length;

  const submit = () => {
    const title = draft.trim();
    if (!title) return;
    setDraft('');
    void addSubtask(task.id, title);
  };

  return (
    <section aria-labelledby={headingId} className="flex flex-col gap-1">
      <h3 id={headingId} className={cn(FIELD_LABEL, 'px-2.5')}>
        Subtasks{task.subtasks.length > 0 && <span className="tabular-nums"> · {done}/{task.subtasks.length}</span>}
      </h3>
      {task.subtasks.length > 0 && (
        <ul className="flex flex-col">
          {task.subtasks.map(sub => {
            const saving = isTempSubtaskId(sub.id);
            return (
              <li key={sub.id} className="group flex items-center gap-2 h-8 px-2.5 rounded-lg hover:bg-shodh-raised">
                <button
                  type="button"
                  role="checkbox"
                  aria-checked={sub.completed}
                  aria-label={sub.title}
                  disabled={saving}
                  onClick={() => void toggleSubtask(task.id, sub.id)}
                  className={cn(
                    'w-4 h-4 shrink-0 rounded border inline-flex items-center justify-center transition-colors duration-micro disabled:opacity-50',
                    sub.completed ? 'bg-shodh-success border-shodh-success text-shodh-ground' : 'border-shodh-border-strong hover:border-shodh-text-muted',
                    FOCUS_RING,
                  )}
                >
                  {sub.completed && <Check className="w-3 h-3" strokeWidth={3} aria-hidden="true" />}
                </button>
                <span className={cn('flex-1 min-w-0 truncate text-[13px]', sub.completed ? 'line-through text-shodh-text-faint' : 'text-shodh-text-secondary')}>
                  {sub.title}
                </span>
                <button
                  type="button"
                  aria-label={`Delete subtask ${sub.title}`}
                  disabled={saving}
                  onClick={() => void deleteSubtask(task.id, sub.id)}
                  className={cn(
                    'w-6 h-6 shrink-0 rounded-md inline-flex items-center justify-center text-shodh-text-muted opacity-0 group-hover:opacity-100 focus-visible:opacity-100 hover:text-shodh-error hover:bg-shodh-pressed transition-opacity duration-micro disabled:hidden',
                    FOCUS_RING,
                  )}
                >
                  <X className="w-3.5 h-3.5" aria-hidden="true" />
                </button>
              </li>
            );
          })}
        </ul>
      )}
      <form
        className="flex items-center gap-2 px-2.5"
        onSubmit={e => { e.preventDefault(); submit(); inputRef.current?.focus(); }}
      >
        <Plus className="w-4 h-4 shrink-0 text-shodh-text-faint" aria-hidden="true" />
        <label htmlFor={inputId} className="sr-only">New subtask</label>
        <input
          ref={inputRef}
          id={inputId}
          type="text"
          value={draft}
          placeholder="Add a subtask"
          onChange={e => setDraft(e.target.value)}
          onKeyDown={e => {
            // The sheet ignores Esc from inputs, so step out of the field here
            // (the next Esc closes the sheet).
            if (e.key === 'Escape') {
              setDraft('');
              e.currentTarget.closest<HTMLElement>('[role="dialog"]')?.focus();
            }
          }}
          className={cn(INLINE_INPUT, 'h-8 text-[13px] px-1.5')}
        />
      </form>
    </section>
  );
}

/** Task detail and editing sheet. Every field saves on its own (blur/Enter/click). */
export default function TaskDetailSheet({ task, onClose }: { task: TodoItem | null; onClose: () => void }) {
  const { updateTask, deleteTask, tasks } = useTasksStore();
  // Keep showing the last task while the sheet animates closed.
  const last = useRef<TodoItem | null>(null);
  if (task) last.current = task;
  const shown = task ?? last.current;
  const projectListId = useId();

  if (!shown) return null;
  const projects = Array.from(new Set(tasks.map(t => t.project).filter((p): p is string => !!p))).sort();

  return (
    <DetailSheet
      open={task !== null}
      onClose={onClose}
      title={`Task: ${shown.title}`}
      kindLabel="Task"
      footer={
        <>
          <span className="mr-auto text-[11px] text-shodh-text-faint">
            Created {formatStamp(shown.createdAt)}
            {shown.updatedAt !== shown.createdAt && <> · Edited {formatStamp(shown.updatedAt)}</>}
          </span>
          <button
            type="button"
            onClick={() => deleteTask(shown.id)}
            className={cn(
              'h-8 px-3 inline-flex items-center gap-1.5 rounded-lg text-[12.5px] text-shodh-error hover:bg-shodh-raised transition-colors duration-micro',
              FOCUS_RING,
            )}
          >
            <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
            Delete
          </button>
        </>
      }
    >
      <InlineText
        key={`title-${shown.id}`}
        label="Title"
        labelHidden
        required
        value={shown.title}
        onCommit={title => void updateTask(shown.id, { title })}
        className="h-auto py-1.5 text-[17px] font-semibold"
      />
      <Provenance source={shown.source} sourceRef={shown.sourceRef} />
      <div className="px-2.5 flex flex-wrap gap-x-6 gap-y-3">
        <Segmented
          label="Status"
          value={shown.status}
          options={STATUSES.map(s => ({ value: s, label: STATUS_LABELS[s] }))}
          onChange={status => void updateTask(shown.id, { status })}
        />
        <Segmented
          label="Priority"
          value={shown.priority}
          options={PRIORITIES.map(p => ({ value: p, label: PRIORITY_LABELS[p], selectedClass: PRIORITY_SELECTED[p] }))}
          onChange={priority => void updateTask(shown.id, { priority })}
        />
      </div>
      <div className="px-2.5">
        <DateTimeField
          key={`due-${shown.id}`}
          label="Due"
          value={shown.dueDate}
          emptyHint="No due date. Pick a date; a time is optional."
          onCommit={dueDate => void updateTask(shown.id, { dueDate })}
        />
      </div>
      <InlineText
        key={`notes-${shown.id}`}
        label="Notes"
        multiline
        value={shown.description}
        placeholder="Add notes"
        hint="Saves when you leave the field, or press Ctrl+Enter."
        onCommit={description => void updateTask(shown.id, { description })}
      />
      <div className="flex flex-col gap-1">
        <InlineText
          key={`project-${shown.id}`}
          label="Project"
          value={shown.project ?? ''}
          placeholder="No project"
          list={projectListId}
          required={!!shown.project}
          hint={shown.project ? 'A project can be changed but not removed yet.' : undefined}
          onCommit={project => void updateTask(shown.id, { project })}
        />
        <datalist id={projectListId}>
          {projects.map(p => <option key={p} value={p} />)}
        </datalist>
      </div>
      <div className="px-1">
        <ChipInput
          key={`tags-${shown.id}`}
          label="Tags"
          tags={shown.tags}
          onChange={tags => void updateTask(shown.id, { tags })}
        />
      </div>
      <SubtaskList task={shown} />
    </DetailSheet>
  );
}
