import { useEffect, useId, useRef, useState } from 'react';
import type React from 'react';
import { Check, ChevronDown, FilePen, ListChecks, ShieldCheck, Zap } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { cn } from '../../lib/utils';
import { IS_MAC_PLATFORM, ariaKeyShortcuts, withShortcut } from '../../lib/shortcuts';
import { Kbd } from '../../components/ui/Kbd';
import { AGENT_MODES, MODE_FLOOR, agentModeInfo } from './agentModes';
import type { AgentMode } from './agentModes';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

/** Each mode's icon, shown left of its name. */
export const MODE_ICONS: Record<AgentMode, LucideIcon> = {
  ask: ShieldCheck,
  acceptEdits: FilePen,
  plan: ListChecks,
  auto: Zap,
};

interface ModePickerProps {
  mode: AgentMode;
  /** The user picked a mode (the agent never calls this). */
  onChange: (mode: AgentMode) => void;
  /** An answer is running: a change applies to the next one. */
  answerRunning: boolean;
}

/**
 * The conversation's mode in the composer, always visible: what runs without asking.
 * Opens the list of modes; Shift+Tab in the composer picks the next one.
 */
export function ModePicker({ mode, onChange, answerRunning }: ModePickerProps) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuId = useId();
  const current = agentModeInfo(mode);
  const Icon = MODE_ICONS[mode];

  useEffect(() => {
    if (!open) return;
    const items = menuRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]');
    const selected = menuRef.current?.querySelector<HTMLButtonElement>('[aria-checked="true"]');
    (selected ?? items?.[0])?.focus();
    const onPointer = (e: PointerEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('pointerdown', onPointer);
    return () => document.removeEventListener('pointerdown', onPointer);
  }, [open]);

  const close = (restore: boolean) => {
    setOpen(false);
    if (restore) buttonRef.current?.focus();
  };

  const pick = (next: AgentMode) => {
    close(true);
    if (next !== mode) onChange(next);
  };

  const onMenuKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const items = Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]') ?? []);
    const index = items.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      close(true);
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      items[(index + 1) % items.length]?.focus();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      items[(index - 1 + items.length) % items.length]?.focus();
    } else if (e.key === 'Home' || e.key === 'End') {
      e.preventDefault();
      items[e.key === 'Home' ? 0 : items.length - 1]?.focus();
    } else if (e.key === 'Tab') {
      close(false);
    }
  };

  const title = withShortcut(
    `Mode: ${current.label}. ${current.hint}${answerRunning ? ' A change applies to the next answer.' : ''} Switch the mode`,
    'cycleMode',
    IS_MAC_PLATFORM,
  );

  return (
    <div ref={rootRef} className="relative shrink-0">
      <button
        ref={buttonRef}
        type="button"
        onClick={() => setOpen(o => !o)}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        aria-label={`Mode: ${current.label}`}
        aria-keyshortcuts={ariaKeyShortcuts('cycleMode', IS_MAC_PLATFORM)}
        title={title}
        data-agent-mode={mode}
        className={cn(
          'inline-flex items-center gap-1 h-[30px] pl-2.5 pr-2 rounded-full text-caption font-medium transition-colors duration-micro',
          mode === 'ask'
            ? 'bg-shodh-raised-2 text-shodh-text-secondary hover:text-shodh-text'
            : 'bg-shodh-accent/10 text-shodh-accent hover:bg-shodh-accent/15',
          FOCUS_RING,
        )}
      >
        <Icon className="w-3 h-3 shrink-0" aria-hidden="true" />
        {current.label}
        <ChevronDown className="w-3 h-3 shrink-0 opacity-70" aria-hidden="true" />
      </button>
      {open && (
        <div className="shell-pop absolute left-0 bottom-full mb-1.5 z-50 w-[320px] max-w-[calc(100vw-32px)] rounded-lg border border-shodh-border-strong bg-shodh-raised shadow-lg overflow-hidden">
          <div id={menuId} ref={menuRef} role="menu" aria-label="Mode" onKeyDown={onMenuKeyDown} className="py-1">
            {AGENT_MODES.map(info => {
              const ItemIcon = MODE_ICONS[info.id];
              const selected = info.id === mode;
              return (
                <button
                  key={info.id}
                  type="button"
                  role="menuitemradio"
                  aria-checked={selected}
                  onClick={() => pick(info.id)}
                  className="w-full flex items-start gap-2 px-3 py-2 text-left hover:bg-shodh-raised-2 focus-visible:bg-shodh-raised-2 focus-visible:outline-none"
                >
                  <ItemIcon className="w-3.5 h-3.5 mt-0.5 shrink-0 text-shodh-text-secondary" aria-hidden="true" />
                  <span className="flex-1 min-w-0 flex flex-col gap-0.5">
                    <span className="text-small font-medium text-shodh-text">{info.label}</span>
                    <span className="text-caption text-shodh-text-muted">{info.hint}</span>
                  </span>
                  {selected && <Check className="w-3.5 h-3.5 mt-0.5 shrink-0 text-shodh-accent" aria-hidden="true" />}
                </button>
              );
            })}
          </div>
          <div className="flex flex-col gap-1 px-3 py-2 border-t border-shodh-border-subtle">
            <p className="m-0 text-caption text-shodh-text-muted">{MODE_FLOOR}</p>
            <p className="m-0 flex items-center gap-1.5 text-caption text-shodh-text-muted">
              <Kbd shortcut="cycleMode" />
              <span>in the message box picks the next mode</span>
            </p>
          </div>
        </div>
      )}
    </div>
  );
}

export default ModePicker;
