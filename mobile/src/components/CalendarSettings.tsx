/**
 * Settings, Calendar: calendar sync with this phone's own calendar, opt-in.
 * The user picks which calendars Shodh may read; Shodh's events go into a
 * calendar of its own ("Shodh"). Whether the chosen calendars' events go to
 * the computer at all is a second, plain choice: "Let Shodh use my calendar".
 */
import { useEffect, useState } from 'react';
import { CalendarCheck, CalendarDays, Square, SquareCheck } from 'lucide-react';
import { useAgenda } from '../lib/agenda';
import { useNote } from '../lib/context';
import type { PhoneCalendarInfo } from '../lib/device';
import { errorText } from '../lib/link';
import { useDevice } from '../lib/queue';
import { NO_SETTINGS, SETTINGS_CHANGED, type PhoneSettings } from './Background';

function Switch({ label, checked, onChange, disabled = false }: { label: string; checked: boolean; onChange: (on: boolean) => void; disabled?: boolean }) {
  return (
    <button type="button" role="switch" className="switch" aria-checked={checked} aria-label={label} disabled={disabled} onClick={() => onChange(!checked)}>
      <span className="switch-knob" aria-hidden="true" />
    </button>
  );
}

export function CalendarSettings() {
  const { choice, setChoice } = useAgenda();
  const { calendar } = useDevice();
  const { show } = useNote();
  const [calendars, setCalendars] = useState<PhoneCalendarInfo[] | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!choice.enabled) return;
    let alive = true;
    calendar
      .list()
      .then(list => {
        if (alive) setCalendars(list);
      })
      .catch(e => show(errorText(e), 'problem'));
    return () => {
      alive = false;
    };
  }, [choice.enabled, calendar, show]);

  const save = (next: typeof choice) => {
    setSaving(true);
    setChoice(next)
      .catch(e => show(errorText(e), 'problem'))
      .finally(() => setSaving(false));
  };

  const toggle = (id: number) =>
    save({ ...choice, chosen: choice.chosen.includes(id) ? choice.chosen.filter(c => c !== id) : [...choice.chosen, id] });

  return (
    <div className="card">
      <div className="setting setting-switch">
        <span className="row-text">
          <span className="row-title">Sync with this phone's calendar</span>
          <span className="row-sub">Shodh's events go into a calendar of its own, called Shodh. Edits go both ways; a delete asks you first.</span>
        </span>
        <Switch label="Sync with this phone's calendar" checked={choice.enabled} disabled={saving} onChange={on => save({ ...choice, enabled: on })} />
      </div>
      {choice.enabled && (
        <>
          <div className="setting">
            <span className="row-title">Show these calendars in Shodh</span>
            <span className="row-sub">Only the calendars you tick are read. The others never leave this phone.</span>
            <div className="choices" role="group" aria-label="Calendars to show in Shodh">
              {(calendars ?? [])
                .filter(c => !c.shodh)
                .map(c => {
                  const on = choice.chosen.includes(c.id);
                  return (
                    <button key={c.id} type="button" role="checkbox" className="choice" aria-checked={on} disabled={saving} onClick={() => toggle(c.id)}>
                      {on ? <SquareCheck aria-hidden="true" className="icon-small" /> : <Square aria-hidden="true" className="icon-small" />}
                      <span className="row-text">
                        <span className="row-title">{c.name}</span>
                        {c.account && <span className="row-sub">{c.account}</span>}
                      </span>
                      <span className="calendar-swatch" style={{ background: c.color }} aria-hidden="true" />
                    </button>
                  );
                })}
              {calendars !== null && calendars.filter(c => !c.shodh).length === 0 && <p className="row-sub">This phone has no other calendars.</p>}
            </div>
          </div>
          <div className="setting setting-switch">
            <span className="row-text">
              <span className="row-title">
                <CalendarCheck aria-hidden="true" className="icon-small" /> Let Shodh use my calendar
              </span>
              <span className="row-sub">
                {choice.useWithShodh
                  ? 'On: events of the calendars you ticked go to Shodh on your computer, where search and the assistant can use them (for example, to say when you are free). They travel only over your encrypted link.'
                  : 'Off: events of the calendars you ticked show here only. They are not sent to your computer, not searched, and the assistant never sees them.'}
              </span>
            </span>
            <Switch label="Let Shodh use my calendar" checked={choice.useWithShodh} disabled={saving} onChange={on => save({ ...choice, useWithShodh: on })} />
          </div>
          {choice.useWithShodh && <BriefMinutes />}
        </>
      )}
      {!choice.enabled && (
        <div className="setting">
          <span className="row-sub">
            <CalendarDays aria-hidden="true" className="icon-small" /> Off. Shodh asks for permission to use your calendar when you turn it on, and you can turn it off at any time.
          </span>
        </div>
      )}
    </div>
  );
}

const BRIEF_CHOICES: ReadonlyArray<{ value: number | null; label: string }> = [
  { value: null, label: 'Off' },
  { value: 5, label: '5 min' },
  { value: 10, label: '10 min' },
  { value: 30, label: '30 min' },
];

/** "A brief before your meetings": off unless the user picks how long before. */
function BriefMinutes() {
  const { sealed } = useDevice();
  const [minutes, setMinutes] = useState<number | null>(null);
  useEffect(() => {
    sealed
      .read<PhoneSettings>('settings')
      .then(saved => setMinutes(saved?.briefMinutes ?? null))
      .catch(() => undefined);
  }, [sealed]);
  const choose = (value: number | null) => {
    setMinutes(value);
    sealed
      .read<PhoneSettings>('settings')
      .then(saved => sealed.write('settings', { ...NO_SETTINGS, ...(saved ?? {}), briefMinutes: value }))
      .then(() => window.dispatchEvent(new Event(SETTINGS_CHANGED)))
      .catch(() => undefined);
  };
  return (
    <div className="setting">
      <span className="row-title">A brief before your meetings</span>
      <span className="row-sub">Shodh tells you, this long before a meeting, when your library or your conversations have something on it. Nothing is shown for meetings nothing matches.</span>
      <div className="segmented segmented-4" role="radiogroup" aria-label="A brief before your meetings">
        {BRIEF_CHOICES.map(choice => (
          <button key={choice.label} type="button" role="radio" className="segment" aria-checked={minutes === choice.value} onClick={() => choose(choice.value)}>
            {choice.label}
          </button>
        ))}
      </div>
    </div>
  );
}
