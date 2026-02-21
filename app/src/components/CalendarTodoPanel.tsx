import React, { useState, useEffect, useMemo, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { motion, AnimatePresence } from 'framer-motion';
import {
  Plus, Check, Trash2, ChevronLeft, ChevronRight, Clock,
  Bot, User, FileText, AlertCircle, Calendar as CalendarIcon,
  CircleDot, Flag, Tag, Loader2, CheckCircle2, FolderOpen,
  ListTodo, ChevronDown, ChevronUp, X,
} from 'lucide-react';
import { useTheme } from '../contexts/ThemeContext';

// ── Types ────────────────────────────────────────────────────────

interface SubTask {
  id: string;
  title: string;
  completed: boolean;
}

interface TodoItem {
  id: string;
  title: string;
  description: string;
  dueDate: string | null;
  priority: string;
  status: string;
  tags: string[];
  subtasks: SubTask[];
  project: string | null;
  source: string;
  sourceRef: string | null;
  createdAt: string;
  updatedAt: string;
  completedAt: string | null;
  reminder: string | null;
}

interface CalendarEvent {
  id: string;
  title: string;
  description: string;
  startTime: string;
  endTime: string | null;
  allDay: boolean;
  color: string | null;
  source: string;
  sourceRef: string | null;
  createdAt: string;
}

type FilterTab = 'all' | 'pending' | 'completed';

// ── Helpers ──────────────────────────────────────────────────────

const PRIORITY_COLORS: Record<string, string> = {
  high: '#ef4444',
  medium: '#f59e0b',
  low: '#10b981',
};

const SOURCE_ICONS: Record<string, React.ElementType> = {
  user: User,
  agent: Bot,
  document: FileText,
};

function isOverdue(dueDate: string | null): boolean {
  if (!dueDate) return false;
  return new Date(dueDate) < new Date() && new Date(dueDate).toDateString() !== new Date().toDateString();
}

function formatDate(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleDateString([], { month: 'short', day: 'numeric', year: 'numeric' });
}

function formatTime(iso: string): string {
  return new Date(iso).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

function isSameDay(d1: Date, d2: Date): boolean {
  return d1.getFullYear() === d2.getFullYear() &&
    d1.getMonth() === d2.getMonth() &&
    d1.getDate() === d2.getDate();
}

/** Quick date helpers for the "Today", "Tomorrow", "Next Week" buttons */
function toLocalDatetimeStr(d: Date): string {
  const pad = (n: number) => n.toString().padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function getQuickDates() {
  const today = new Date();
  today.setHours(17, 0, 0, 0); // 5 PM today
  const tomorrow = new Date(today);
  tomorrow.setDate(tomorrow.getDate() + 1);
  const nextWeek = new Date(today);
  nextWeek.setDate(nextWeek.getDate() + 7);
  return {
    today: toLocalDatetimeStr(today),
    tomorrow: toLocalDatetimeStr(tomorrow),
    nextWeek: toLocalDatetimeStr(nextWeek),
  };
}

function friendlyDueLabel(iso: string): string {
  const d = new Date(iso);
  const now = new Date();
  const diffMs = d.getTime() - now.getTime();
  const diffDays = Math.ceil(diffMs / (1000 * 60 * 60 * 24));

  if (isSameDay(d, now)) return 'Today';
  if (diffDays === 1) return 'Tomorrow';
  if (diffDays > 1 && diffDays <= 7) return d.toLocaleDateString([], { weekday: 'short' });
  return d.toLocaleDateString([], { month: 'short', day: 'numeric' });
}

// ── Calendar Grid ────────────────────────────────────────────────

function MiniCalendar({
  tasks,
  events,
  selectedDate,
  onSelectDate,
}: {
  tasks: TodoItem[];
  events: CalendarEvent[];
  selectedDate: Date | null;
  onSelectDate: (d: Date | null) => void;
}) {
  const { colors } = useTheme();
  const [viewDate, setViewDate] = useState(new Date());

  const year = viewDate.getFullYear();
  const month = viewDate.getMonth();
  const firstDay = new Date(year, month, 1).getDay();
  const daysInMonth = new Date(year, month + 1, 0).getDate();
  const today = new Date();

  // Build set of dates that have tasks or events
  const activeDates = useMemo(() => {
    const set = new Set<string>();
    tasks.forEach(t => {
      if (t.dueDate) {
        set.add(new Date(t.dueDate).toDateString());
      }
    });
    events.forEach(e => {
      set.add(new Date(e.startTime).toDateString());
    });
    return set;
  }, [tasks, events]);

  const days: (number | null)[] = [];
  for (let i = 0; i < firstDay; i++) days.push(null);
  for (let d = 1; d <= daysInMonth; d++) days.push(d);

  const prevMonth = () => setViewDate(new Date(year, month - 1, 1));
  const nextMonth = () => setViewDate(new Date(year, month + 1, 1));

  const monthLabel = viewDate.toLocaleDateString([], { month: 'long', year: 'numeric' });

  return (
    <div>
      {/* Month header */}
      <div className="flex items-center justify-between mb-3">
        <button onClick={prevMonth} className="p-1 rounded hover:opacity-70 transition-opacity">
          <ChevronLeft className="w-4 h-4" style={{ color: colors.textMuted }} />
        </button>
        <span className="text-xs font-semibold" style={{ color: colors.text }}>{monthLabel}</span>
        <button onClick={nextMonth} className="p-1 rounded hover:opacity-70 transition-opacity">
          <ChevronRight className="w-4 h-4" style={{ color: colors.textMuted }} />
        </button>
      </div>

      {/* Day headers */}
      <div className="grid grid-cols-7 gap-0.5 mb-1">
        {['Su', 'Mo', 'Tu', 'We', 'Th', 'Fr', 'Sa'].map(d => (
          <div key={d} className="text-center text-[10px] font-medium py-0.5" style={{ color: colors.textMuted }}>
            {d}
          </div>
        ))}
      </div>

      {/* Day grid */}
      <div className="grid grid-cols-7 gap-0.5">
        {days.map((day, i) => {
          if (day === null) return <div key={`empty-${i}`} />;

          const date = new Date(year, month, day);
          const isToday = isSameDay(date, today);
          const isSelected = selectedDate && isSameDay(date, selectedDate);
          const hasItems = activeDates.has(date.toDateString());

          return (
            <button
              key={day}
              onClick={() => {
                if (isSelected) {
                  onSelectDate(null);
                } else {
                  onSelectDate(date);
                }
              }}
              className="relative flex flex-col items-center justify-center py-1 rounded-md transition-all text-[11px]"
              style={{
                backgroundColor: isSelected
                  ? `${colors.primary}20`
                  : isToday
                    ? `${colors.primary}08`
                    : 'transparent',
                color: isSelected ? colors.primary : isToday ? colors.primary : colors.text,
                fontWeight: isToday || isSelected ? 600 : 400,
                border: isToday ? `1px solid ${colors.primary}30` : '1px solid transparent',
              }}
            >
              {day}
              {hasItems && (
                <div
                  className="w-1 h-1 rounded-full mt-0.5"
                  style={{ backgroundColor: isSelected ? colors.primary : colors.accent }}
                />
              )}
            </button>
          );
        })}
      </div>
    </div>
  );
}

// ── Add Task Form ────────────────────────────────────────────────

function AddTaskForm({
  onAdd,
  onCancel,
  projects,
  defaultDate,
}: {
  onAdd: (task: TodoItem) => void;
  onCancel: () => void;
  projects: string[];
  defaultDate?: Date | null;
}) {
  const { colors } = useTheme();
  const [title, setTitle] = useState('');
  const [dueDate, setDueDate] = useState(() => {
    if (defaultDate) {
      const d = new Date(defaultDate);
      d.setHours(17, 0, 0, 0);
      return toLocalDatetimeStr(d);
    }
    return '';
  });
  const [priority, setPriority] = useState('medium');
  const [project, setProject] = useState('');
  const [newProject, setNewProject] = useState('');
  const [showNewProject, setShowNewProject] = useState(false);
  const [submitting, setSubmitting] = useState(false);

  const handleSubmit = async () => {
    if (!title.trim()) return;
    setSubmitting(true);
    try {
      const assignProject = showNewProject ? (newProject.trim() || null) : (project || null);
      const task = await invoke<TodoItem>('create_task', {
        title: title.trim(),
        dueDate: dueDate || null,
        priority,
        project: assignProject,
        source: 'user',
      });
      onAdd(task);
      setTitle('');
      setDueDate('');
      setPriority('medium');
      setProject('');
      setNewProject('');
      setShowNewProject(false);
    } catch (err) {
      console.error('Failed to create task:', err);
    } finally {
      setSubmitting(false);
    }
  };

  const quickDates = useMemo(() => getQuickDates(), []);
  const [showDatePicker, setShowDatePicker] = useState(false);

  return (
    <motion.div
      initial={{ opacity: 0, height: 0 }}
      animate={{ opacity: 1, height: 'auto' }}
      exit={{ opacity: 0, height: 0 }}
      className="overflow-hidden"
    >
      <div
        className="p-3 rounded-lg border space-y-3"
        style={{ borderColor: colors.border, backgroundColor: colors.cardBg }}
      >
        {/* Title input */}
        <input
          type="text"
          value={title}
          onChange={e => setTitle(e.target.value)}
          placeholder="What do you need to do?"
          className="w-full bg-transparent outline-none text-sm font-medium"
          style={{ color: colors.text }}
          autoFocus
          onKeyDown={e => {
            if (e.key === 'Enter' && title.trim()) handleSubmit();
            if (e.key === 'Escape') onCancel();
          }}
        />

        {/* Quick date buttons */}
        <div className="flex items-center gap-1.5 flex-wrap">
          <span className="text-[10px] mr-1" style={{ color: colors.textMuted }}>Due:</span>
          {[
            { label: 'Today', value: quickDates.today },
            { label: 'Tomorrow', value: quickDates.tomorrow },
            { label: 'Next Week', value: quickDates.nextWeek },
          ].map(opt => (
            <button
              key={opt.label}
              onClick={() => { setDueDate(opt.value); setShowDatePicker(false); }}
              className="text-[10px] px-2 py-1 rounded-md font-medium transition-all"
              style={{
                backgroundColor: dueDate === opt.value ? `${colors.primary}15` : `${colors.border}40`,
                color: dueDate === opt.value ? colors.primary : colors.textSecondary,
                border: `1px solid ${dueDate === opt.value ? colors.primary + '30' : 'transparent'}`,
              }}
            >
              {opt.label}
            </button>
          ))}
          <button
            onClick={() => setShowDatePicker(!showDatePicker)}
            className="text-[10px] px-2 py-1 rounded-md font-medium transition-all flex items-center gap-1"
            style={{
              backgroundColor: showDatePicker || (dueDate && dueDate !== quickDates.today && dueDate !== quickDates.tomorrow && dueDate !== quickDates.nextWeek)
                ? `${colors.primary}15` : `${colors.border}40`,
              color: showDatePicker ? colors.primary : colors.textSecondary,
            }}
          >
            <CalendarIcon className="w-3 h-3" />
            {dueDate && dueDate !== quickDates.today && dueDate !== quickDates.tomorrow && dueDate !== quickDates.nextWeek
              ? friendlyDueLabel(dueDate)
              : 'Pick date'}
          </button>
          {dueDate && (
            <button
              onClick={() => setDueDate('')}
              className="p-0.5 rounded hover:opacity-70"
              title="Clear date"
            >
              <X className="w-3 h-3" style={{ color: colors.textMuted }} />
            </button>
          )}
        </div>

        {/* Date picker (expanded) */}
        <AnimatePresence>
          {showDatePicker && (
            <motion.div
              initial={{ opacity: 0, height: 0 }}
              animate={{ opacity: 1, height: 'auto' }}
              exit={{ opacity: 0, height: 0 }}
              className="overflow-hidden"
            >
              <input
                type="datetime-local"
                value={dueDate}
                onChange={e => setDueDate(e.target.value)}
                className="text-[11px] px-2 py-1.5 rounded border bg-transparent outline-none w-full"
                style={{ borderColor: colors.border, color: colors.textSecondary }}
              />
            </motion.div>
          )}
        </AnimatePresence>

        {/* Priority + Project row */}
        <div className="flex items-center gap-2 flex-wrap">
          <div className="flex gap-1">
            {(['low', 'medium', 'high'] as const).map(p => (
              <button
                key={p}
                onClick={() => setPriority(p)}
                className="text-[10px] px-2 py-0.5 rounded-full font-medium transition-all"
                style={{
                  backgroundColor: priority === p ? `${PRIORITY_COLORS[p]}20` : 'transparent',
                  color: priority === p ? PRIORITY_COLORS[p] : colors.textMuted,
                  border: `1px solid ${priority === p ? PRIORITY_COLORS[p] + '40' : 'transparent'}`,
                }}
              >
                {p}
              </button>
            ))}
          </div>

          <div className="w-px h-4" style={{ backgroundColor: colors.border }} />

          {/* Project selector */}
          {showNewProject ? (
            <div className="flex items-center gap-1">
              <input
                type="text"
                value={newProject}
                onChange={e => setNewProject(e.target.value)}
                placeholder="New project..."
                className="text-[11px] px-2 py-1 rounded border bg-transparent outline-none w-28"
                style={{ borderColor: colors.border, color: colors.textSecondary }}
                onKeyDown={e => {
                  if (e.key === 'Escape') { setShowNewProject(false); setNewProject(''); }
                }}
                autoFocus
              />
              <button
                onClick={() => { setShowNewProject(false); setNewProject(''); }}
                className="p-0.5 rounded hover:opacity-70"
              >
                <X className="w-3 h-3" style={{ color: colors.textMuted }} />
              </button>
            </div>
          ) : (
            <div className="flex items-center gap-1">
              <FolderOpen className="w-3 h-3" style={{ color: colors.textMuted }} />
              <select
                value={project}
                onChange={e => {
                  if (e.target.value === '__new__') {
                    setShowNewProject(true);
                    setProject('');
                  } else {
                    setProject(e.target.value);
                  }
                }}
                className="text-[11px] px-1.5 py-1 rounded border bg-transparent outline-none"
                style={{ borderColor: colors.border, color: colors.textSecondary }}
              >
                <option value="">No project</option>
                {projects.map(p => (
                  <option key={p} value={p}>{p}</option>
                ))}
                <option value="__new__">+ New project</option>
              </select>
            </div>
          )}

          <div className="flex-1" />

          <button
            onClick={onCancel}
            className="text-[11px] px-2 py-1 rounded transition-colors"
            style={{ color: colors.textMuted }}
          >
            Cancel
          </button>
          <button
            onClick={handleSubmit}
            disabled={!title.trim() || submitting}
            className="text-[11px] px-3 py-1 rounded font-medium text-white disabled:opacity-40 transition-all"
            style={{ backgroundColor: colors.primary }}
          >
            {submitting ? <Loader2 className="w-3 h-3 animate-spin" /> : 'Add Task'}
          </button>
        </div>
      </div>
    </motion.div>
  );
}

// ── Task Row ─────────────────────────────────────────────────────

function TaskRow({
  task,
  onToggle,
  onDelete,
  onUpdate,
}: {
  task: TodoItem;
  onToggle: (id: string, status: string) => void;
  onDelete: (id: string) => void;
  onUpdate: (updated: TodoItem) => void;
}) {
  const { colors, theme } = useTheme();
  const [expanded, setExpanded] = useState(false);
  const [addingSubtask, setAddingSubtask] = useState(false);
  const [subtaskTitle, setSubtaskTitle] = useState('');
  const isDone = task.status === 'completed';
  const overdue = !isDone && isOverdue(task.dueDate);
  const priorityColor = PRIORITY_COLORS[task.priority] || PRIORITY_COLORS.medium;
  const completedSubtasks = (task.subtasks || []).filter(s => s.completed).length;
  const totalSubtasks = (task.subtasks || []).length;

  const handleAddSubtask = async () => {
    if (!subtaskTitle.trim()) return;
    try {
      const updated = await invoke<TodoItem>('add_subtask', {
        taskId: task.id,
        title: subtaskTitle.trim(),
      });
      onUpdate(updated);
      setSubtaskTitle('');
      setAddingSubtask(false);
    } catch (err) {
      console.error('Failed to add subtask:', err);
    }
  };

  const handleToggleSubtask = async (subtaskId: string) => {
    try {
      const updated = await invoke<TodoItem>('toggle_subtask', {
        taskId: task.id,
        subtaskId,
      });
      onUpdate(updated);
    } catch (err) {
      console.error('Failed to toggle subtask:', err);
    }
  };

  const handleDeleteSubtask = async (subtaskId: string) => {
    try {
      const updated = await invoke<TodoItem>('delete_subtask', {
        taskId: task.id,
        subtaskId,
      });
      onUpdate(updated);
    } catch (err) {
      console.error('Failed to delete subtask:', err);
    }
  };

  return (
    <motion.div
      layout
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, x: -20 }}
      className="group rounded-lg transition-colors"
      style={{
        backgroundColor: 'transparent',
        borderLeft: overdue ? `3px solid ${PRIORITY_COLORS.high}` : `3px solid transparent`,
      }}
      onMouseEnter={e => (e.currentTarget.style.backgroundColor = theme === 'dark' ? 'rgba(255,255,255,0.03)' : 'rgba(0,0,0,0.02)')}
      onMouseLeave={e => (e.currentTarget.style.backgroundColor = 'transparent')}
    >
      <div className="flex items-start gap-2.5 px-3 py-2.5">
        {/* Checkbox */}
        <button
          onClick={() => onToggle(task.id, isDone ? 'pending' : 'completed')}
          className="mt-0.5 w-4 h-4 rounded-full border-2 flex items-center justify-center shrink-0 transition-all"
          style={{
            borderColor: isDone ? '#10b981' : colors.border,
            backgroundColor: isDone ? '#10b981' : 'transparent',
          }}
        >
          {isDone && <Check className="w-2.5 h-2.5 text-white" />}
        </button>

        {/* Content */}
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-1.5">
            <button
              onClick={() => setExpanded(!expanded)}
              className="text-[13px] font-medium truncate text-left"
              style={{
                color: isDone ? colors.textMuted : colors.text,
                textDecoration: isDone ? 'line-through' : 'none',
                opacity: isDone ? 0.6 : 1,
              }}
            >
              {task.title}
            </button>
            {task.source === 'agent' && (
              <Bot className="w-3 h-3 shrink-0" style={{ color: colors.primary }} title="Created by AI" />
            )}
            {totalSubtasks > 0 && (
              <button
                onClick={() => setExpanded(!expanded)}
                className="flex items-center gap-0.5 text-[10px] px-1.5 py-0.5 rounded-full"
                style={{ backgroundColor: `${colors.primary}10`, color: colors.primary }}
              >
                <ListTodo className="w-2.5 h-2.5" />
                {completedSubtasks}/{totalSubtasks}
                {expanded ? <ChevronUp className="w-2.5 h-2.5" /> : <ChevronDown className="w-2.5 h-2.5" />}
              </button>
            )}
          </div>

          {/* Meta line */}
          <div className="flex items-center gap-2 mt-0.5 flex-wrap">
            {task.dueDate && (
              <span
                className="text-[10px] flex items-center gap-1"
                style={{ color: overdue ? PRIORITY_COLORS.high : colors.textMuted }}
                title={formatDate(task.dueDate)}
              >
                <Clock className="w-3 h-3" />
                {friendlyDueLabel(task.dueDate)}
                {overdue && ' · overdue'}
              </span>
            )}
            <span
              className="w-1.5 h-1.5 rounded-full shrink-0"
              style={{ backgroundColor: priorityColor }}
              title={`Priority: ${task.priority}`}
            />
            {task.project && (
              <span
                className="text-[10px] flex items-center gap-1 px-1.5 py-0.5 rounded-full"
                style={{ backgroundColor: `${colors.accent}15`, color: colors.accent }}
              >
                <FolderOpen className="w-2.5 h-2.5" />
                {task.project}
              </span>
            )}
            {task.tags.length > 0 && (
              <div className="flex items-center gap-1">
                {task.tags.slice(0, 2).map(tag => (
                  <span
                    key={tag}
                    className="text-[9px] px-1.5 py-0.5 rounded-full"
                    style={{ backgroundColor: `${colors.primary}10`, color: colors.primary }}
                  >
                    {tag}
                  </span>
                ))}
                {task.tags.length > 2 && (
                  <span className="text-[9px]" style={{ color: colors.textMuted }}>
                    +{task.tags.length - 2}
                  </span>
                )}
              </div>
            )}
          </div>

          {task.description && (
            <p className="text-[11px] mt-1 line-clamp-2" style={{ color: colors.textMuted }}>
              {task.description}
            </p>
          )}

          {/* Subtask progress bar */}
          {totalSubtasks > 0 && !expanded && (
            <div className="mt-1.5 h-1 rounded-full overflow-hidden" style={{ backgroundColor: `${colors.border}` }}>
              <div
                className="h-full rounded-full transition-all"
                style={{
                  width: `${(completedSubtasks / totalSubtasks) * 100}%`,
                  backgroundColor: completedSubtasks === totalSubtasks ? '#10b981' : colors.primary,
                }}
              />
            </div>
          )}
        </div>

        {/* Actions */}
        <div className="flex items-center gap-1 shrink-0">
          <button
            onClick={() => setExpanded(!expanded)}
            className="opacity-0 group-hover:opacity-100 transition-opacity p-1 rounded hover:opacity-70"
            title={expanded ? 'Collapse' : 'Expand subtasks'}
          >
            {expanded
              ? <ChevronUp className="w-3 h-3" style={{ color: colors.textMuted }} />
              : <ChevronDown className="w-3 h-3" style={{ color: colors.textMuted }} />
            }
          </button>
          <button
            onClick={() => onDelete(task.id)}
            className="opacity-0 group-hover:opacity-100 transition-opacity p-1 rounded hover:bg-red-500/10"
            title="Delete task"
          >
            <Trash2 className="w-3 h-3" style={{ color: '#ef4444' }} />
          </button>
        </div>
      </div>

      {/* Expanded: Subtasks */}
      <AnimatePresence>
        {expanded && (
          <motion.div
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: 'auto' }}
            exit={{ opacity: 0, height: 0 }}
            className="overflow-hidden"
          >
            <div className="pl-10 pr-3 pb-2.5 space-y-1">
              {(task.subtasks || []).map(sub => (
                <div
                  key={sub.id}
                  className="group/sub flex items-center gap-2 py-1 px-2 rounded-md transition-colors"
                  onMouseEnter={e => (e.currentTarget.style.backgroundColor = theme === 'dark' ? 'rgba(255,255,255,0.03)' : 'rgba(0,0,0,0.02)')}
                  onMouseLeave={e => (e.currentTarget.style.backgroundColor = 'transparent')}
                >
                  <button
                    onClick={() => handleToggleSubtask(sub.id)}
                    className="w-3.5 h-3.5 rounded border flex items-center justify-center shrink-0 transition-all"
                    style={{
                      borderColor: sub.completed ? '#10b981' : colors.border,
                      backgroundColor: sub.completed ? '#10b981' : 'transparent',
                    }}
                  >
                    {sub.completed && <Check className="w-2 h-2 text-white" />}
                  </button>
                  <span
                    className="flex-1 text-[12px]"
                    style={{
                      color: sub.completed ? colors.textMuted : colors.textSecondary,
                      textDecoration: sub.completed ? 'line-through' : 'none',
                    }}
                  >
                    {sub.title}
                  </span>
                  <button
                    onClick={() => handleDeleteSubtask(sub.id)}
                    className="opacity-0 group-hover/sub:opacity-100 transition-opacity p-0.5 rounded hover:bg-red-500/10"
                  >
                    <X className="w-2.5 h-2.5" style={{ color: '#ef4444' }} />
                  </button>
                </div>
              ))}

              {/* Add subtask */}
              {addingSubtask ? (
                <div className="flex items-center gap-2 py-1 px-2">
                  <Plus className="w-3.5 h-3.5 shrink-0" style={{ color: colors.textMuted }} />
                  <input
                    type="text"
                    value={subtaskTitle}
                    onChange={e => setSubtaskTitle(e.target.value)}
                    placeholder="Subtask title..."
                    className="flex-1 text-[12px] bg-transparent outline-none"
                    style={{ color: colors.text }}
                    autoFocus
                    onKeyDown={e => {
                      if (e.key === 'Enter' && subtaskTitle.trim()) handleAddSubtask();
                      if (e.key === 'Escape') { setAddingSubtask(false); setSubtaskTitle(''); }
                    }}
                  />
                  <button
                    onClick={handleAddSubtask}
                    disabled={!subtaskTitle.trim()}
                    className="text-[10px] px-2 py-0.5 rounded font-medium text-white disabled:opacity-40"
                    style={{ backgroundColor: colors.primary }}
                  >
                    Add
                  </button>
                  <button
                    onClick={() => { setAddingSubtask(false); setSubtaskTitle(''); }}
                    className="p-0.5"
                  >
                    <X className="w-3 h-3" style={{ color: colors.textMuted }} />
                  </button>
                </div>
              ) : (
                <button
                  onClick={() => setAddingSubtask(true)}
                  className="flex items-center gap-2 py-1 px-2 text-[11px] rounded-md transition-colors w-full"
                  style={{ color: colors.textMuted }}
                  onMouseEnter={e => (e.currentTarget.style.color = colors.primary)}
                  onMouseLeave={e => (e.currentTarget.style.color = colors.textMuted)}
                >
                  <Plus className="w-3 h-3" />
                  Add subtask
                </button>
              )}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </motion.div>
  );
}

// ── Main Panel ───────────────────────────────────────────────────

export default function CalendarTodoPanel() {
  const { colors, theme } = useTheme();
  const [tasks, setTasks] = useState<TodoItem[]>([]);
  const [events, setEvents] = useState<CalendarEvent[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState<FilterTab>('all');
  const [showAddForm, setShowAddForm] = useState(false);
  const [selectedDate, setSelectedDate] = useState<Date | null>(null);
  const [projectFilter, setProjectFilter] = useState<string | null>(null);

  const fetchData = useCallback(async () => {
    try {
      const [t, e] = await Promise.all([
        invoke<TodoItem[]>('load_tasks'),
        invoke<CalendarEvent[]>('load_events'),
      ]);
      setTasks(t);
      setEvents(e);
      setError(null);
    } catch (err) {
      setError(`Failed to load data: ${err}`);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchData();
  }, [fetchData]);

  const handleToggle = async (id: string, status: string) => {
    try {
      const updated = await invoke<TodoItem>('update_task', { id, status });
      setTasks(prev => prev.map(t => t.id === id ? updated : t));
    } catch (err) {
      console.error('Failed to update task:', err);
    }
  };

  const handleDelete = async (id: string) => {
    try {
      await invoke('delete_task', { id });
      setTasks(prev => prev.filter(t => t.id !== id));
    } catch (err) {
      console.error('Failed to delete task:', err);
    }
  };

  const handleAddTask = (task: TodoItem) => {
    setTasks(prev => [task, ...prev]);
    setShowAddForm(false);
  };

  const handleUpdate = (updated: TodoItem) => {
    setTasks(prev => prev.map(t => t.id === updated.id ? updated : t));
  };

  // Collect unique project names across all tasks
  const allProjects = useMemo(() => {
    const set = new Set<string>();
    tasks.forEach(t => { if (t.project) set.add(t.project); });
    return Array.from(set).sort();
  }, [tasks]);

  // Filtered + sorted tasks
  const filteredTasks = useMemo(() => {
    let result = [...tasks];

    // Filter by status tab
    if (filter === 'pending') result = result.filter(t => t.status !== 'completed');
    if (filter === 'completed') result = result.filter(t => t.status === 'completed');

    // Filter by selected calendar date
    if (selectedDate) {
      result = result.filter(t => {
        if (!t.dueDate) return false;
        return isSameDay(new Date(t.dueDate), selectedDate);
      });
    }

    // Filter by project
    if (projectFilter) {
      result = result.filter(t => t.project === projectFilter);
    }

    // Sort: overdue first, then by due date, then by created date
    result.sort((a, b) => {
      if (a.status === 'completed' && b.status !== 'completed') return 1;
      if (a.status !== 'completed' && b.status === 'completed') return -1;

      const aOverdue = isOverdue(a.dueDate);
      const bOverdue = isOverdue(b.dueDate);
      if (aOverdue && !bOverdue) return -1;
      if (!aOverdue && bOverdue) return 1;

      if (a.dueDate && b.dueDate) return new Date(a.dueDate).getTime() - new Date(b.dueDate).getTime();
      if (a.dueDate) return -1;
      if (b.dueDate) return 1;

      return new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime();
    });

    return result;
  }, [tasks, filter, selectedDate, projectFilter]);

  // Stats
  const pendingCount = tasks.filter(t => t.status !== 'completed').length;
  const completedCount = tasks.filter(t => t.status === 'completed').length;
  const overdueCount = tasks.filter(t => t.status !== 'completed' && isOverdue(t.dueDate)).length;

  if (loading) {
    return (
      <div className="h-full flex items-center justify-center">
        <div className="text-center space-y-3">
          <Loader2
            className="w-6 h-6 animate-spin mx-auto"
            style={{ color: colors.primary }}
          />
          <p className="text-sm" style={{ color: colors.textMuted }}>Loading tasks...</p>
        </div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="h-full flex items-center justify-center">
        <div className="text-center space-y-3">
          <AlertCircle className="w-6 h-6 mx-auto" style={{ color: colors.error }} />
          <p className="text-sm" style={{ color: colors.textSecondary }}>{error}</p>
          <button
            onClick={fetchData}
            className="text-xs px-3 py-1.5 rounded-md border transition-colors"
            style={{ borderColor: colors.border, color: colors.textSecondary }}
          >
            Retry
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="h-full overflow-hidden flex flex-col">
      {/* Header */}
      <div className="px-6 pt-5 pb-3 flex items-center justify-between shrink-0">
        <div>
          <h1 className="text-lg font-bold" style={{ color: colors.text }}>Tasks</h1>
          <p className="text-[11px] mt-0.5" style={{ color: colors.textMuted }}>
            {pendingCount} pending
            {overdueCount > 0 && (
              <span style={{ color: PRIORITY_COLORS.high }}> · {overdueCount} overdue</span>
            )}
            {completedCount > 0 && ` · ${completedCount} completed`}
          </p>
        </div>
        <button
          onClick={() => setShowAddForm(!showAddForm)}
          className="flex items-center gap-1.5 text-[11px] px-3 py-1.5 rounded-lg font-medium text-white transition-all"
          style={{ backgroundColor: colors.primary }}
        >
          <Plus className="w-3.5 h-3.5" />
          Add Task
        </button>
      </div>

      {/* Content — split layout */}
      <div className="flex-1 overflow-hidden flex gap-0 px-6 pb-6">
        {/* Left: Task list */}
        <div className="flex-1 flex flex-col overflow-hidden min-w-0">
          {/* Filter tabs */}
          <div className="flex gap-1 mb-3 shrink-0">
            {([
              { id: 'all' as FilterTab, label: 'All', count: tasks.length },
              { id: 'pending' as FilterTab, label: 'Pending', count: pendingCount },
              { id: 'completed' as FilterTab, label: 'Done', count: completedCount },
            ]).map(tab => (
              <button
                key={tab.id}
                onClick={() => setFilter(tab.id)}
                className="text-[11px] px-3 py-1 rounded-full font-medium transition-all"
                style={{
                  backgroundColor: filter === tab.id ? `${colors.primary}15` : 'transparent',
                  color: filter === tab.id ? colors.primary : colors.textMuted,
                }}
              >
                {tab.label}
                <span className="ml-1 tabular-nums" style={{ opacity: 0.7 }}>{tab.count}</span>
              </button>
            ))}
            {/* Active filters */}
            <div className="flex items-center gap-1 ml-auto">
              {projectFilter && (
                <button
                  onClick={() => setProjectFilter(null)}
                  className="text-[10px] px-2 py-1 rounded-full flex items-center gap-1 transition-colors"
                  style={{ backgroundColor: `${colors.accent}10`, color: colors.accent }}
                >
                  <FolderOpen className="w-3 h-3" />
                  {projectFilter}
                  <span className="ml-0.5">×</span>
                </button>
              )}
              {selectedDate && (
                <button
                  onClick={() => setSelectedDate(null)}
                  className="text-[10px] px-2 py-1 rounded-full flex items-center gap-1 transition-colors"
                  style={{ backgroundColor: `${colors.primary}10`, color: colors.primary }}
                >
                  <CalendarIcon className="w-3 h-3" />
                  {selectedDate.toLocaleDateString([], { month: 'short', day: 'numeric' })}
                  <span className="ml-0.5">×</span>
                </button>
              )}
            </div>
          </div>

          {/* Add task form */}
          <AnimatePresence>
            {showAddForm && (
              <div className="mb-3 shrink-0">
                <AddTaskForm
                  onAdd={handleAddTask}
                  onCancel={() => setShowAddForm(false)}
                  projects={allProjects}
                  defaultDate={selectedDate}
                />
              </div>
            )}
          </AnimatePresence>

          {/* Task list */}
          <div className="flex-1 overflow-y-auto -mx-1">
            <AnimatePresence mode="popLayout">
              {filteredTasks.length === 0 ? (
                <motion.div
                  initial={{ opacity: 0 }}
                  animate={{ opacity: 1 }}
                  className="flex flex-col items-center justify-center py-12 text-center"
                >
                  <CheckCircle2
                    className="w-10 h-10 mb-3"
                    style={{ color: colors.textMuted, opacity: 0.3 }}
                  />
                  <p className="text-sm font-medium" style={{ color: colors.textSecondary }}>
                    {filter === 'completed' ? 'No completed tasks' :
                     selectedDate ? 'No tasks on this date' :
                     'No tasks yet'}
                  </p>
                  <p className="text-[11px] mt-1" style={{ color: colors.textMuted }}>
                    {filter === 'all' && !selectedDate
                      ? 'Add tasks manually or ask your AI assistant to create them'
                      : 'Try a different filter or date'}
                  </p>
                </motion.div>
              ) : (
                filteredTasks.map(task => (
                  <TaskRow
                    key={task.id}
                    task={task}
                    onToggle={handleToggle}
                    onDelete={handleDelete}
                    onUpdate={handleUpdate}
                  />
                ))
              )}
            </AnimatePresence>
          </div>
        </div>

        {/* Right: Calendar + Events */}
        <div
          className="w-64 shrink-0 ml-4 pl-4 flex flex-col gap-4"
          style={{ borderLeft: `1px solid ${colors.border}` }}
        >
          {/* Mini calendar */}
          <div
            className="p-3 rounded-lg border"
            style={{ borderColor: colors.border, backgroundColor: colors.cardBg }}
          >
            <MiniCalendar
              tasks={tasks}
              events={events}
              selectedDate={selectedDate}
              onSelectDate={setSelectedDate}
            />
          </div>

          {/* Projects */}
          {allProjects.length > 0 && (
            <div>
              <h3 className="text-[11px] font-semibold uppercase tracking-wider mb-2" style={{ color: colors.textMuted }}>
                Projects
              </h3>
              <div className="space-y-1">
                {allProjects.map(proj => {
                  const count = tasks.filter(t => t.project === proj && t.status !== 'completed').length;
                  const isActive = projectFilter === proj;
                  return (
                    <button
                      key={proj}
                      onClick={() => setProjectFilter(isActive ? null : proj)}
                      className="w-full flex items-center gap-2 px-2.5 py-1.5 rounded-md text-left transition-all text-[11px]"
                      style={{
                        backgroundColor: isActive ? `${colors.accent}15` : 'transparent',
                        color: isActive ? colors.accent : colors.textSecondary,
                      }}
                    >
                      <FolderOpen className="w-3 h-3 shrink-0" />
                      <span className="flex-1 truncate font-medium">{proj}</span>
                      <span className="tabular-nums" style={{ opacity: 0.7 }}>{count}</span>
                    </button>
                  );
                })}
              </div>
            </div>
          )}

          {/* Upcoming events */}
          <div>
            <h3 className="text-[11px] font-semibold uppercase tracking-wider mb-2" style={{ color: colors.textMuted }}>
              Events
            </h3>
            {events.length === 0 ? (
              <p className="text-[11px]" style={{ color: colors.textMuted }}>
                No events yet
              </p>
            ) : (
              <div className="space-y-1.5">
                {events
                  .sort((a, b) => new Date(a.startTime).getTime() - new Date(b.startTime).getTime())
                  .slice(0, 8)
                  .map(event => (
                    <div
                      key={event.id}
                      className="flex items-start gap-2 px-2.5 py-2 rounded-md"
                      style={{
                        backgroundColor: `${event.color || colors.primary}08`,
                        borderLeft: `2px solid ${event.color || colors.primary}`,
                      }}
                    >
                      <div className="flex-1 min-w-0">
                        <p className="text-[11px] font-medium truncate" style={{ color: colors.text }}>
                          {event.title}
                        </p>
                        <p className="text-[10px]" style={{ color: colors.textMuted }}>
                          {formatDate(event.startTime)}
                          {!event.allDay && ` ${formatTime(event.startTime)}`}
                        </p>
                      </div>
                      {event.source === 'agent' && (
                        <Bot className="w-3 h-3 shrink-0 mt-0.5" style={{ color: colors.primary }} />
                      )}
                    </div>
                  ))}
              </div>
            )}
          </div>

          {/* Quick stats */}
          <div
            className="p-3 rounded-lg border space-y-2"
            style={{ borderColor: colors.border, backgroundColor: colors.cardBg }}
          >
            <h3 className="text-[11px] font-semibold uppercase tracking-wider" style={{ color: colors.textMuted }}>
              Overview
            </h3>
            <div className="space-y-1.5">
              {[
                { label: 'Total tasks', value: tasks.length, color: colors.text },
                { label: 'Pending', value: pendingCount, color: '#f59e0b' },
                { label: 'Overdue', value: overdueCount, color: '#ef4444' },
                { label: 'Completed', value: completedCount, color: '#10b981' },
                { label: 'Events', value: events.length, color: colors.primary },
              ].map(stat => (
                <div key={stat.label} className="flex items-center justify-between">
                  <span className="text-[11px]" style={{ color: colors.textMuted }}>{stat.label}</span>
                  <span className="text-[12px] font-semibold tabular-nums" style={{ color: stat.color }}>
                    {stat.value}
                  </span>
                </div>
              ))}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
