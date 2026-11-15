/**
 * Tasks on the phone: today's agenda and what is coming up, the task list
 * with a tick to finish one and a date to move it, and a quick add for a task
 * or an event. Everything is the computer's (its own Tasks and calendar);
 * while it is away the phone shows its sealed copy and keeps the user's edits
 * to send when it wakes. Events of the phone calendars the user chose show
 * too, marked as the phone's.
 */
import { useState, type FormEvent } from 'react';
import { AnimatePresence, motion } from 'framer-motion';
import { CalendarDays, CalendarPlus, Circle, NotebookText, Plus, Trash2, Undo2 } from 'lucide-react';
import { BriefSheet } from '../components/BriefSheet';
import { briefsAllowed } from '../lib/brief';
import { TopBar } from '../components/Bars';
import { NotConnected } from '../components/Connection';
import { QueuedRows } from '../components/Queued';
import { useAgenda, type Task } from '../lib/agenda';
import { desktopFields, phoneFields, type DesktopEvent, type Fields, type PhoneEvent } from '../lib/calsync';
import { useLink, useNote } from '../lib/context';
import { useOffers } from '../lib/incoming';
import { errorText } from '../lib/link';
import { viewIn } from '../lib/motion';
import { canQueue, useQueue } from '../lib/queue';

/** One line of the agenda. */
interface AgendaItem {
  key: string;
  title: string;
  fields: Fields;
  /** From a phone calendar, not Shodh's. */
  phone: boolean;
}

const pad = (n: number) => String(n).padStart(2, '0');

/** `YYYY-MM-DD` of `date`, local time. */
export function dayKey(date: Date): string {
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** "09:30", or "All day". */
export function timeWords(fields: Fields): string {
  if (fields.allDay) return 'All day';
  const time = fields.start.slice(11, 16);
  const end = fields.end?.slice(11, 16);
  return end && fields.end?.slice(0, 10) === fields.start.slice(0, 10) ? `${time} to ${end}` : time;
}

/** The agenda from `from` (a day) for `days` days, Shodh's events and the phone's, in order. */
export function agendaItems(events: DesktopEvent[], phone: PhoneEvent[], from: Date, days: number): AgendaItem[] {
  const first = dayKey(from);
  const last = dayKey(new Date(from.getFullYear(), from.getMonth(), from.getDate() + days - 1));
  const items: AgendaItem[] = [
    ...events.map(e => ({ key: `s:${e.id}`, title: e.title, fields: desktopFields(e), phone: false })),
    ...phone.map(e => ({ key: `p:${e.calendarId}:${e.eventId}`, title: e.title, fields: phoneFields(e), phone: true })),
  ];
  return items
    .filter(item => {
      const day = item.fields.start.slice(0, 10);
      return day >= first && day <= last;
    })
    .sort((a, b) => a.fields.start.localeCompare(b.fields.start) || a.title.localeCompare(b.title));
}

function done(task: Task): boolean {
  return task.status === 'completed';
}

/** "Due 14 Nov", "Due today", "Overdue". */
export function dueWords(task: Task, today: Date): string | null {
  if (!task.dueDate) return null;
  const day = task.dueDate.slice(0, 10);
  const key = dayKey(today);
  if (day === key) return 'Due today';
  if (day < key && !done(task)) return 'Overdue';
  const [y, m, d] = day.split('-').map(Number);
  return `Due ${new Date(y, m - 1, d).toLocaleDateString(undefined, { day: 'numeric', month: 'short' })}`;
}

export function Tasks({ now = () => new Date() }: { now?: () => Date }) {
  const { connected, status, desktop } = useLink();
  const agenda = useAgenda();
  const queue = useQueue();
  const { show } = useNote();
  const [newTask, setNewTask] = useState('');
  const [newDue, setNewDue] = useState('');
  const [eventTitle, setEventTitle] = useState('');
  const [eventDate, setEventDate] = useState('');
  const [eventTime, setEventTime] = useState('');
  const today = now();
  const queueable = canQueue(status);
  const [brief, setBrief] = useState<{ eventId: string; title: string } | null>(null);
  const offers = useOffers();
  const closeOpened = offers.closeOpened;
  const openedEvent = offers.opened ? agenda.events?.find(e => e.id === offers.opened) : undefined;
  // Briefs read the calendar: only with "Let Shodh use my calendar", and from the computer.
  const onBrief = connected && briefsAllowed(agenda.choice) ? (eventId: string, title: string) => setBrief({ eventId, title }) : undefined;

  /** The user's own edit: sent now, or kept until the computer wakes. */
  const edit = async (command: string, args: Record<string, unknown>, label: string) => {
    const id = crypto.randomUUID();
    const withRef = command === 'create_task' || command === 'create_event' ? { ...args, sourceRef: `phone:${id}` } : args;
    try {
      if (connected) {
        await desktop.invoke(command, withRef);
        agenda.reload();
      } else if (queueable) {
        await queue.add({ kind: 'edit', command, args: withRef, label }, id);
        show('Saved on your phone. It is sent when your computer wakes.');
      } else {
        show('Connect to your computer to change your tasks.', 'problem');
      }
    } catch (e) {
      show(errorText(e), 'problem');
    }
  };

  const addTask = (event: FormEvent) => {
    event.preventDefault();
    const title = newTask.trim();
    if (!title) return;
    setNewTask('');
    setNewDue('');
    void edit('create_task', { title, ...(newDue ? { dueDate: newDue } : {}) }, `Add the task “${title}”`);
  };

  const addEvent = (event: FormEvent) => {
    event.preventDefault();
    const title = eventTitle.trim();
    if (!title || !eventDate) return;
    setEventTitle('');
    setEventTime('');
    const startTime = eventTime ? `${eventDate}T${eventTime}` : eventDate;
    void edit('create_event', { title, startTime, allDay: !eventTime }, `Add the event “${title}”`);
  };

  const tasks = agenda.tasks ?? [];
  const open = tasks.filter(t => !done(t)).sort((a, b) => (a.dueDate ?? '9999').localeCompare(b.dueDate ?? '9999'));
  const finished = tasks.filter(done).length;
  const todayItems = agenda.events ? agendaItems(agenda.events, agenda.phoneEvents, today, 1) : [];
  const tomorrow = new Date(today.getFullYear(), today.getMonth(), today.getDate() + 1);
  const coming = agenda.events ? agendaItems(agenda.events, agenda.phoneEvents, tomorrow, 7) : [];
  const edits = queue.items.filter(i => i.work.kind === 'edit' && i.state !== 'answered');
  const canEdit = connected || queueable;

  return (
    <motion.div className="view" variants={viewIn} initial="hidden" animate="shown" exit="gone">
      <div className="scroll">
        <TopBar />
        <h1 className="title pad">Tasks</h1>
        {agenda.fromCopy && agenda.savedAt !== null && (
          <p className="line line-waiting pad">
            {`Your computer is away. This is your agenda as of ${new Date(agenda.savedAt).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })}; what you change is sent when it wakes.`}
          </p>
        )}
        {agenda.tasks === null && !connected && <NotConnected what="your tasks and agenda" />}
        {agenda.tasks === null && connected && (
          <div className="skeleton-lines" aria-busy="true" aria-label="Loading your tasks">
            <div className="skeleton skeleton-line" style={{ width: '80%' }} />
            <div className="skeleton skeleton-line" style={{ width: '64%' }} />
          </div>
        )}

        {openedEvent && (
          <div className="callout" role="status" style={{ margin: '16px 20px 0' }}>
            <CalendarDays aria-hidden="true" className="icon" />
            <div className="row-text">
              <p className="row-title">{openedEvent.title}</p>
              <p className="row-sub">{`Opened from your phone’s calendar · ${desktopFields(openedEvent).start.replace('T', ' ')}`}</p>
              <button type="button" className="button" onClick={closeOpened}>
                <Undo2 aria-hidden="true" className="icon-small" />
                Done
              </button>
            </div>
          </div>
        )}
        {offers.opened && agenda.events !== null && !openedEvent && (
          <p className="panel-text panel-quiet">This event is no longer in Shodh.</p>
        )}
        {agenda.pending.map(question => (
          <div key={question.id} className="callout" role="alert" style={{ margin: '16px 20px 0' }}>
            <Trash2 aria-hidden="true" className="icon" />
            <div className="row-text">
              <p>
                {question.deletedOn === 'phone'
                  ? `“${question.title}” was removed from your phone’s calendar. Remove it from Shodh too?`
                  : `“${question.title}” was removed in Shodh. Remove it from your phone’s calendar too?`}
              </p>
              <div className="sheet-actions">
                <button type="button" className="button" onClick={() => void agenda.answer(question.id, false).catch(e => show(errorText(e), 'problem'))}>
                  <Undo2 aria-hidden="true" className="icon-small" />
                  Keep it
                </button>
                <button type="button" className="button button-primary" onClick={() => void agenda.answer(question.id, true).catch(e => show(errorText(e), 'problem'))}>
                  <Trash2 aria-hidden="true" className="icon-small" />
                  Remove
                </button>
              </div>
            </div>
          </div>
        ))}

        {agenda.events !== null && (
          <>
            <div className="label">
              <span>Today</span>
              <span className="label-note">{today.toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long' })}</span>
            </div>
            <AgendaRows items={todayItems} empty="Nothing on your calendar today." onBrief={onBrief} />
            {coming.length > 0 && (
              <>
                <div className="label">Coming up</div>
                <AgendaRows items={coming} empty="" withDay onBrief={onBrief} />
              </>
            )}
          </>
        )}

        {agenda.tasks !== null && (
          <>
            <div className="label">
              <span>To do</span>
              {finished > 0 && <span className="label-note">{`${finished} done`}</span>}
            </div>
            {open.length === 0 ? (
              <p className="panel-text panel-quiet">Nothing to do. Add a task below.</p>
            ) : (
              <div className="rows rows-tasks">
                {open.map(task => {
                  const due = dueWords(task, today);
                  return (
                    <div key={task.id} className="row task">
                      <button
                        type="button"
                        className="icon-button"
                        role="checkbox"
                        aria-checked={false}
                        aria-label={`Tick off “${task.title}”`}
                        disabled={!canEdit}
                        onClick={() => void edit('update_task', { id: task.id, status: 'completed' }, `Tick off “${task.title}”`)}
                      >
                        <Circle aria-hidden="true" className="icon" />
                      </button>
                      <span className="row-text">
                        <span className="row-title">{task.title}</span>
                        {due && <span className={due === 'Overdue' ? 'row-sub row-alert' : 'row-sub'}>{due}</span>}
                      </span>
                      <label className="date-field">
                        <span className="visually-hidden">{`Due date for “${task.title}”`}</span>
                        <input
                          type="date"
                          value={task.dueDate?.slice(0, 10) ?? ''}
                          disabled={!canEdit}
                          onChange={event => {
                            const value = event.target.value;
                            void edit(
                              'update_task',
                              value ? { id: task.id, dueDate: value } : { id: task.id, clear: ['due_date'] },
                              `Move “${task.title}”`,
                            );
                          }}
                        />
                      </label>
                    </div>
                  );
                })}
              </div>
            )}
          </>
        )}

        {edits.length > 0 && (
          <>
            <div className="label">Waiting for your computer</div>
            <QueuedRows items={edits} />
          </>
        )}

        {canEdit && (
          <>
            <div className="label">Add a task</div>
            <form className="quick-add" onSubmit={addTask}>
              <label htmlFor="new-task" className="visually-hidden">
                The new task
              </label>
              <input id="new-task" value={newTask} onChange={e => setNewTask(e.target.value)} placeholder="What needs doing?" autoComplete="off" />
              <label htmlFor="new-task-due" className="visually-hidden">
                Its due date
              </label>
              <input id="new-task-due" type="date" value={newDue} onChange={e => setNewDue(e.target.value)} />
              <button type="submit" className="button button-primary" disabled={newTask.trim() === ''}>
                <Plus aria-hidden="true" className="icon-small" />
                Add
              </button>
            </form>

            <div className="label">Add an event</div>
            <form className="quick-add" onSubmit={addEvent}>
              <label htmlFor="new-event" className="visually-hidden">
                The new event
              </label>
              <input id="new-event" value={eventTitle} onChange={e => setEventTitle(e.target.value)} placeholder="What is it?" autoComplete="off" />
              <label htmlFor="new-event-date" className="visually-hidden">
                Its day
              </label>
              <input id="new-event-date" type="date" value={eventDate} onChange={e => setEventDate(e.target.value)} />
              <label htmlFor="new-event-time" className="visually-hidden">
                Its time (none: all day)
              </label>
              <input id="new-event-time" type="time" value={eventTime} onChange={e => setEventTime(e.target.value)} />
              <button type="submit" className="button button-primary" disabled={eventTitle.trim() === '' || eventDate === ''}>
                <CalendarPlus aria-hidden="true" className="icon-small" />
                Add
              </button>
            </form>
          </>
        )}
        {agenda.syncError && <p className="panel-text panel-quiet">{`Calendar sync stopped: ${agenda.syncError}`}</p>}
      </div>
      <AnimatePresence>{brief && <BriefSheet key={brief.eventId} eventId={brief.eventId} title={brief.title} onClose={() => setBrief(null)} />}</AnimatePresence>
    </motion.div>
  );
}

function AgendaRows({
  items,
  empty,
  withDay = false,
  onBrief,
}: {
  items: AgendaItem[];
  empty: string;
  withDay?: boolean;
  /** A brief before a meeting, for Shodh's events (with the user's calendar consent). */
  onBrief?: (eventId: string, title: string) => void;
}) {
  if (items.length === 0) return empty ? <p className="panel-text panel-quiet">{empty}</p> : null;
  return (
    <div className="rows rows-agenda">
      {items.map(item => {
        const [y, m, d] = item.fields.start.slice(0, 10).split('-').map(Number);
        const day = withDay ? `${new Date(y, m - 1, d).toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' })} · ` : '';
        return (
          <div key={item.key} className="row agenda-row" data-source={item.phone ? 'phone' : 'shodh'}>
            <span className="agenda-bar" aria-hidden="true" />
            <span className="row-text">
              <span className="row-title">{item.title}</span>
              <span className="row-sub">{`${day}${timeWords(item.fields)}${item.phone ? ' · your phone’s calendar' : ''}`}</span>
            </span>
            {onBrief && !item.phone && (
              <button type="button" className="button" onClick={() => onBrief(item.key.slice(2), item.title)}>
                <NotebookText aria-hidden="true" className="icon-small" />
                Brief
              </button>
            )}
          </div>
        );
      })}
    </div>
  );
}
