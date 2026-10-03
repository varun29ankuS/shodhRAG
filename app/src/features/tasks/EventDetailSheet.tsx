import React, { useRef } from 'react';
import { Trash2 } from 'lucide-react';
import { cn } from '../../lib/utils';
import DetailSheet from './DetailSheet';
import { dayDelta, shiftDays, storedDayKey, storedTime } from './dueDate';
import { DateTimeField, FOCUS_RING, InlineText, Segmented } from './fields';
import { Provenance } from './TaskDetailSheet';
import { useTasksStore } from './TasksStore';
import type { EventPatch } from './TasksStore';
import type { CalendarEvent } from './types';

/** Start/end after switching all-day on or off. Timed events default to 09:00–10:00. */
export function allDayPatch(event: CalendarEvent, allDay: boolean): EventPatch {
  const startDay = storedDayKey(event.startTime);
  const endDay = storedDayKey(event.endTime);
  if (!startDay) return { allDay };
  if (allDay) {
    return { allDay, startTime: startDay, ...(endDay ? { endTime: endDay } : {}) };
  }
  return {
    allDay,
    startTime: `${startDay}T09:00`,
    ...(endDay ? { endTime: `${endDay}T${endDay === startDay ? '10:00' : '09:00'}` } : {}),
  };
}

/**
 * New start, moving the end by the same number of days so the event keeps
 * its span; if the end would still precede the start it is set to the start.
 */
export function startPatch(event: CalendarEvent, startTime: string): EventPatch {
  const oldDay = storedDayKey(event.startTime);
  const newDay = storedDayKey(startTime);
  if (!event.endTime || !oldDay || !newDay) return { startTime };
  const delta = dayDelta(oldDay, newDay) ?? 0;
  let endTime = delta === 0 ? event.endTime : shiftDays(event.endTime, delta) ?? event.endTime;
  const start = storedTime(startTime);
  const end = storedTime(endTime);
  if (start !== null && end !== null && end < start) endTime = startTime;
  return { startTime, endTime };
}

export default function EventDetailSheet({ event, onClose }: { event: CalendarEvent | null; onClose: () => void }) {
  const { updateEvent, deleteEvent } = useTasksStore();
  const last = useRef<CalendarEvent | null>(null);
  if (event) last.current = event;
  const shown = event ?? last.current;
  if (!shown) return null;

  const validateEnd = (next: string) => {
    const start = storedTime(shown.startTime);
    const end = storedTime(next);
    return start !== null && end !== null && end < start ? 'The end cannot be before the start.' : null;
  };

  return (
    <DetailSheet
      open={event !== null}
      onClose={onClose}
      title={`Event: ${shown.title}`}
      kindLabel="Event"
      footer={
        <button
          type="button"
          onClick={() => deleteEvent(shown.id)}
          className={cn(
            'ml-auto h-8 px-3 inline-flex items-center gap-1.5 rounded-lg text-[12.5px] text-shodh-error hover:bg-shodh-raised transition-colors duration-micro',
            FOCUS_RING,
          )}
        >
          <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
          Delete
        </button>
      }
    >
      <InlineText
        key={`title-${shown.id}`}
        label="Title"
        labelHidden
        required
        value={shown.title}
        onCommit={title => void updateEvent(shown.id, { title })}
        className="h-auto py-1.5 text-[17px] font-semibold"
      />
      <Provenance source={shown.source} sourceRef={shown.sourceRef} />
      <div className="px-2.5 flex flex-col gap-3">
        <Segmented
          label="Timing"
          value={shown.allDay ? 'allday' : 'timed'}
          options={[{ value: 'timed', label: 'At a time' }, { value: 'allday', label: 'All day' }]}
          onChange={v => void updateEvent(shown.id, allDayPatch(shown, v === 'allday'))}
        />
        <DateTimeField
          key={`start-${shown.id}-${shown.allDay}`}
          label="Starts"
          value={shown.startTime}
          allowTime={!shown.allDay}
          onCommit={startTime => void updateEvent(shown.id, startPatch(shown, startTime))}
        />
        <DateTimeField
          key={`end-${shown.id}-${shown.allDay}`}
          label="Ends"
          value={shown.endTime}
          allowTime={!shown.allDay}
          emptyHint="No end set."
          validate={validateEnd}
          onCommit={endTime => void updateEvent(shown.id, { endTime })}
          onClear={() => void updateEvent(shown.id, { endTime: null })}
        />
      </div>
      <InlineText
        key={`location-${shown.id}`}
        label="Location"
        value={shown.location ?? ''}
        placeholder="Add a place, address or link"
        onCommit={location => void updateEvent(shown.id, { location: location || null })}
      />
      <InlineText
        key={`notes-${shown.id}`}
        label="Notes"
        multiline
        value={shown.description}
        placeholder="Add notes"
        hint="Saves when you leave the field, or press Ctrl+Enter."
        onCommit={description => void updateEvent(shown.id, { description })}
      />
    </DetailSheet>
  );
}
