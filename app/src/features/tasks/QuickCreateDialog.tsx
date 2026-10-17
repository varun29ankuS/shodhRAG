import React, { useId, useRef, useState } from 'react';
import * as Dialog from '@radix-ui/react-dialog';
import { Loader2, X } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { FOCUS_RING, INPUT, PRIMARY_BUTTON, QUIET_BUTTON } from '../workspaces/ui';
import { DATE_INPUT, FIELD_LABEL, Segmented } from './fields';
import { TasksStoreProvider, useTasksStore } from './TasksStore';
import { dueLabel, fullLabel } from './dueDate';
import { openInTasks } from './openInTasks';
import {
  defaultEventDraft,
  defaultTaskDraft,
  eventDraftProblem,
  eventInput,
  REMINDER_IN_PAST,
  shownProblem,
  taskDraftProblem,
  taskInput,
  withStart,
} from './quickCreate';
import type { EventDraft, TaskDraft } from './quickCreate';

export type QuickCreateKind = 'task' | 'event';

interface QuickCreateDialogProps {
  /** Which dialog is open; null when none is. */
  kind: QuickCreateKind | null;
  onClose: () => void;
  workspaces: readonly { id: string; name: string }[];
  /** The workspace a new task starts in (the active chat's), or null. */
  defaultWorkspaceId: string | null;
}

/**
 * "New task…" and "New event…" over the current view: no navigation to
 * Tasks. The dialog mounts its own Tasks store (not following the agent,
 * like a workspace's board) while open; the Tasks view, if shown, reloads
 * on the backend's `calendar-changed` event.
 */
export function QuickCreateDialog({ kind, onClose, workspaces, defaultWorkspaceId }: QuickCreateDialogProps) {
  if (!kind) return null;
  return (
    <TasksStoreProvider followAgent={false}>
      {kind === 'task' ? (
        <QuickTaskDialog onClose={onClose} workspaces={workspaces} defaultWorkspaceId={defaultWorkspaceId} />
      ) : (
        <QuickEventDialog onClose={onClose} />
      )}
    </TasksStoreProvider>
  );
}

/** The dialog chrome shared by both forms (the CreateWorkspaceDialog pattern). */
function QuickDialogFrame({
  title,
  description,
  onClose,
  onSubmit,
  problem,
  titleTouched,
  onAttempt,
  saving,
  submitLabel,
  children,
}: {
  title: string;
  description: string;
  onClose: () => void;
  onSubmit: () => void;
  /** Why Create is disabled; null when it is enabled. */
  problem: string | null;
  /** The title was typed in or left, or Create was tried: a missing title is reported. */
  titleTouched: boolean;
  /** Enter was pressed in a field: Create was tried, even while it is disabled. */
  onAttempt: () => void;
  saving: boolean;
  submitLabel: string;
  children: React.ReactNode;
}) {
  const descId = useId();
  const problemId = useId();
  const shown = shownProblem(problem, titleTouched);
  // Radix returns focus to a Dialog.Trigger, and there is none here (the New
  // menu or the palette opened the dialog): focus goes back to what had it.
  const [returnFocus] = useState(() => (document.activeElement instanceof HTMLElement ? document.activeElement : null));
  return (
    <Dialog.Root open onOpenChange={open => { if (!open) onClose(); }}>
      <Dialog.Portal>
        <Dialog.Overlay className="ask-fade-in fixed inset-0 z-[55] bg-black/40" />
        <Dialog.Content
          aria-describedby={descId}
          onCloseAutoFocus={e => {
            e.preventDefault();
            if (returnFocus?.isConnected) returnFocus.focus();
          }}
          className="ask-fade-in fixed z-[55] inset-0 m-auto flex flex-col w-[min(460px,calc(100vw-48px))] max-h-[min(640px,calc(100vh-48px))] h-fit rounded-[18px] border border-shodh-border-strong bg-shodh-surface text-shodh-text shadow-[0_24px_80px_rgba(0,0,0,0.45)] focus:outline-none"
        >
          <header className="shrink-0 flex items-start gap-3 px-5 py-4 border-b border-shodh-border-subtle">
            <div className="flex-1 min-w-0">
              <Dialog.Title className="text-[15px] font-semibold">{title}</Dialog.Title>
              <Dialog.Description id={descId} className="text-[12.5px] text-shodh-text-muted">
                {description}
              </Dialog.Description>
            </div>
            <Dialog.Close
              aria-label="Close"
              className={cn('w-8 h-8 inline-flex items-center justify-center rounded-lg text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text', FOCUS_RING)}
            >
              <X className="w-4 h-4" aria-hidden="true" />
            </Dialog.Close>
          </header>
          <form
            className="flex-1 min-h-0 overflow-y-auto scrollbar-thin px-5 py-4 flex flex-col gap-4"
            onSubmit={e => {
              e.preventDefault();
              onSubmit();
            }}
            onKeyDown={e => {
              // A disabled Create blocks the submit, so Enter is caught here.
              if (e.key === 'Enter' && e.target instanceof HTMLInputElement) onAttempt();
            }}
          >
            {children}
            <div className="flex items-center justify-end gap-2 pt-1">
              {/* Always in the DOM so the live region announces each new reason. */}
              <p id={problemId} aria-live="polite" className="m-0 mr-auto text-[12px] text-shodh-text-muted">{shown ?? ''}</p>
              <Dialog.Close className={QUIET_BUTTON} type="button">Cancel</Dialog.Close>
              <button
                type="submit"
                className={PRIMARY_BUTTON}
                disabled={problem !== null || saving}
                aria-describedby={shown ? problemId : undefined}
              >
                {saving && <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
                {submitLabel}
              </button>
            </div>
          </form>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** A labelled native date input with an optional time input (enabled once a date is picked). */
function DateTimeInputs({
  label,
  date,
  time,
  showTime = true,
  timeRef,
  onChange,
}: {
  label: string;
  date: string;
  time: string;
  showTime?: boolean;
  timeRef?: React.Ref<HTMLInputElement>;
  onChange: (next: { date: string; time: string }) => void;
}) {
  const labelId = useId();
  return (
    <div className="flex flex-col gap-1">
      <span id={labelId} className={FIELD_LABEL}>{label}</span>
      <div role="group" aria-labelledby={labelId} className="flex items-center gap-2 flex-wrap">
        <input
          type="date"
          aria-label={`${label} date`}
          value={date}
          onChange={e => onChange({ date: e.target.value, time: e.target.value ? time : '' })}
          className={DATE_INPUT}
        />
        {showTime && (
          <input
            ref={timeRef}
            type="time"
            aria-label={`${label} time`}
            value={time}
            disabled={!date}
            onChange={e => onChange({ date, time: e.target.value })}
            className={DATE_INPUT}
          />
        )}
      </div>
    </div>
  );
}

function TitleInput({
  value,
  onChange,
  onTouched,
  placeholder,
}: {
  value: string;
  onChange: (next: string) => void;
  /** Typed in or left: from then on a missing title is reported. */
  onTouched: () => void;
  placeholder: string;
}) {
  const id = useId();
  return (
    <div className="flex flex-col gap-1">
      <label htmlFor={id} className={FIELD_LABEL}>Title</label>
      <input
        id={id}
        className={INPUT}
        value={value}
        maxLength={200}
        onChange={e => {
          onTouched();
          onChange(e.target.value);
        }}
        onBlur={onTouched}
        placeholder={placeholder}
        autoFocus
        required
      />
    </div>
  );
}

function QuickTaskDialog({
  onClose,
  workspaces,
  defaultWorkspaceId,
}: {
  onClose: () => void;
  workspaces: readonly { id: string; name: string }[];
  defaultWorkspaceId: string | null;
}) {
  const { createTask } = useTasksStore();
  const [draft, setDraft] = useState<TaskDraft>(() => defaultTaskDraft(defaultWorkspaceId));
  const [titleTouched, setTitleTouched] = useState(false);
  const [saving, setSaving] = useState(false);
  const workspaceSelectId = useId();
  const reminderTimeRef = useRef<HTMLInputElement>(null);
  // A reason found only when Create was pressed (a reminder that has passed
  // since it was typed); cleared by the next edit.
  const [submitProblem, setSubmitProblem] = useState<string | null>(null);
  const problem = submitProblem ?? taskDraftProblem(draft);
  const update = (patch: Partial<TaskDraft>) => {
    setSubmitProblem(null);
    setDraft(d => ({ ...d, ...patch }));
  };

  const create = async () => {
    if (saving) return;
    // Checked again now: the time may have passed the reminder since the last edit.
    const now = new Date();
    const found = taskDraftProblem(draft, now);
    const input = found ? null : taskInput(draft, now);
    if (!input) {
      setTitleTouched(true);
      setSubmitProblem(found);
      // Create is disabled now; focus stays in the dialog, on the field to fix.
      if (found === REMINDER_IN_PAST) reminderTimeRef.current?.focus();
      return;
    }
    setSaving(true);
    // On failure the store shows the error notice; the dialog stays open.
    const task = await createTask(input);
    if (!task) {
      setSaving(false);
      return;
    }
    notify.success(`Task added: ${task.title}`, {
      description: task.dueDate ? `Due ${dueLabel(task.dueDate)}` : undefined,
      action: { label: 'Open in Tasks', onClick: () => openInTasks({ taskId: task.id }) },
    });
    onClose();
  };

  return (
    <QuickDialogFrame
      title="New task"
      description="Added to Tasks. You can add notes, tags and subtasks there."
      onClose={onClose}
      onSubmit={() => void create()}
      problem={problem}
      titleTouched={titleTouched}
      onAttempt={() => setTitleTouched(true)}
      saving={saving}
      submitLabel="Create task"
    >
      <TitleInput value={draft.title} onChange={title => update({ title })} onTouched={() => setTitleTouched(true)} placeholder="e.g. Read the survey paper" />
      <DateTimeInputs
        label="Due (optional)"
        date={draft.dueDate}
        time={draft.dueTime}
        onChange={({ date, time }) => update({ dueDate: date, dueTime: time })}
      />
      <DateTimeInputs
        label="Reminder (optional)"
        date={draft.reminderDate}
        time={draft.reminderTime}
        timeRef={reminderTimeRef}
        onChange={({ date, time }) => update({ reminderDate: date, reminderTime: time })}
      />
      {workspaces.length > 0 && (
        <div className="flex flex-col gap-1">
          <label htmlFor={workspaceSelectId} className={FIELD_LABEL}>Workspace (optional)</label>
          <select
            id={workspaceSelectId}
            value={draft.workspaceId ?? ''}
            onChange={e => update({ workspaceId: e.target.value || null })}
            className={cn(
              'h-8 w-full max-w-[20rem] rounded-lg border border-shodh-border bg-shodh-surface px-2 text-[13px] text-shodh-text hover:border-shodh-border-strong',
              FOCUS_RING,
            )}
          >
            <option value="">No workspace</option>
            {workspaces.map(w => (
              <option key={w.id} value={w.id}>{w.name}</option>
            ))}
          </select>
        </div>
      )}
    </QuickDialogFrame>
  );
}

function QuickEventDialog({ onClose }: { onClose: () => void }) {
  const { createEvent } = useTasksStore();
  const [draft, setDraft] = useState<EventDraft>(() => defaultEventDraft());
  // Until the user edits the end, it follows the start.
  const [endEdited, setEndEdited] = useState(false);
  const [titleTouched, setTitleTouched] = useState(false);
  const [saving, setSaving] = useState(false);
  const problem = eventDraftProblem(draft);

  const create = async () => {
    const input = eventInput(draft);
    if (!input || saving) return;
    setSaving(true);
    const event = await createEvent(input);
    if (!event) {
      setSaving(false);
      return;
    }
    notify.success(`Event added: ${event.title}`, {
      description: fullLabel(event.startTime),
      action: { label: 'Open in Tasks', onClick: () => openInTasks({ eventId: event.id }) },
    });
    onClose();
  };

  return (
    <QuickDialogFrame
      title="New event"
      description="Added to the Tasks calendar."
      onClose={onClose}
      onSubmit={() => void create()}
      problem={problem}
      titleTouched={titleTouched}
      onAttempt={() => setTitleTouched(true)}
      saving={saving}
      submitLabel="Create event"
    >
      <TitleInput value={draft.title} onChange={title => setDraft(d => ({ ...d, title }))} onTouched={() => setTitleTouched(true)} placeholder="e.g. Reading group" />
      <Segmented
        label="Timing"
        value={draft.allDay ? 'allday' : 'timed'}
        options={[{ value: 'timed', label: 'At a time' }, { value: 'allday', label: 'All day' }]}
        onChange={v => setDraft(d => withStart({ ...d, allDay: v === 'allday' }, {}, endEdited))}
      />
      <DateTimeInputs
        label="Starts"
        date={draft.startDate}
        time={draft.startTime}
        showTime={!draft.allDay}
        onChange={({ date, time }) => setDraft(d => withStart(d, { startDate: date, startTime: time }, endEdited))}
      />
      <DateTimeInputs
        label="Ends (optional)"
        date={draft.endDate}
        time={draft.endTime}
        showTime={!draft.allDay}
        onChange={({ date, time }) => {
          setEndEdited(true);
          setDraft(d => ({ ...d, endDate: date, endTime: time }));
        }}
      />
    </QuickDialogFrame>
  );
}
