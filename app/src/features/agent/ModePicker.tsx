import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import type React from 'react';
import { createPortal } from 'react-dom';
import { placeMenu } from '../../lib/menuPlacement';
import type { MenuPlacement } from '../../lib/menuPlacement';
import { Check, ChevronDown, Code2, FilePen, Hammer, ListChecks, Lock, Search, ShieldCheck, Zap } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { cn } from '../../lib/utils';
import { IS_MAC_PLATFORM, ariaKeyShortcuts, withShortcut } from '../../lib/shortcuts';
import { Kbd } from '../../components/ui/Kbd';
import { AUTO_CONFIRM_TEXT, CREATOR_CONFIRM_TEXT, agentModeInfo } from './agentModes';
import type { AgentMode } from './agentModes';
import {
  COMPOSER_CONTROL_HEIGHT,
  COMPOSER_TEST_IDS,
  CREATOR_PERMISSIONS_NOTE,
  PERMISSION_MODES,
  WORK_KINDS,
  modeTriggerText,
  permissionsApply,
  workInfo,
} from './composerRow';
import type { WorkKind } from './composerRow';

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-shodh-surface';

/** Each mode's icon, shown left of its name. */
export const MODE_ICONS: Record<AgentMode, LucideIcon> = {
  ask: ShieldCheck,
  acceptEdits: FilePen,
  plan: ListChecks,
  auto: Zap,
  creator: Hammer,
};

/** Each kind of work's icon. */
export const WORK_ICONS: Record<WorkKind, LucideIcon> = {
  research: Search,
  code: Code2,
  creator: Hammer,
};

/** What the menu's confirm row is about: Creator, or the Auto permission. */
type Confirming = { kind: 'work'; work: 'creator' } | { kind: 'permission'; mode: 'auto' };

const ITEM =
  'w-full flex items-center gap-2 h-8 px-3 text-left text-small text-shodh-text hover:bg-shodh-raised-2 focus-visible:bg-shodh-raised-2 focus-visible:outline-none aria-disabled:opacity-45 aria-disabled:cursor-not-allowed aria-disabled:hover:bg-transparent';

interface ModePickerProps {
  /** What the answer works on: Research, Code or Creator. */
  work: WorkKind;
  /** The permission (never Creator); null while the default is read. */
  permission: AgentMode | null;
  /** Code mode in Settings; null until read. Code and Creator say so when it is off. */
  codeAvailable: boolean | null;
  /** The user picked what the answer works on (Creator only after its confirming click). */
  onPickWork: (work: WorkKind) => void;
  /** The user picked a permission (Auto only after its confirming click). */
  onPickPermission: (mode: AgentMode) => void;
  /** An answer is running: a change applies to the next one. */
  answerRunning: boolean;
  /** Why the menu is locked now (the trigger is disabled with it as its tooltip), or null. */
  lockedReason?: string | null;
}

/**
 * The composer's one mode menu: what the answer works on (Research, Code, Creator) and,
 * in the same menu, its permissions (Ask first, Accept edits, Plan, Auto). The
 * permissions section always shows, so the menu never changes shape; in Creator it is
 * disabled with the reason. Pure (props only), so the floating chat renders it too.
 */
export function ModePicker({ work, permission, codeAvailable, onPickWork, onPickPermission, answerRunning, lockedReason = null }: ModePickerProps) {
  const [open, setOpen] = useState(false);
  // Creator and Auto are turned on only after one more click that says what they do.
  const [confirming, setConfirming] = useState<Confirming | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  // The menu is portaled to the body (fixed), so no scrolling or clipped container
  // around the composer cuts it off; it is placed beside the trigger on each open.
  const menuPanelRef = useRef<HTMLDivElement>(null);
  const [placement, setPlacement] = useState<MenuPlacement | null>(null);
  const menuId = useId();
  const permissionsLabelId = useId();
  const locked = lockedReason !== null;

  useEffect(() => {
    if (!open) {
      setConfirming(null);
      setPlacement(null);
    }
  }, [open]);
  // A lock that starts while the menu is open closes it.
  useEffect(() => {
    if (locked) setOpen(false);
  }, [locked]);
  // Measured before paint, and again when the confirm row changes its height or the
  // window is resized: above the trigger when it fits, else below, else capped.
  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const anchor = buttonRef.current?.getBoundingClientRect();
      const panel = menuPanelRef.current;
      if (!anchor || !panel) return;
      setPlacement(placeMenu(anchor, panel.scrollHeight, panel.offsetWidth, window.innerWidth, window.innerHeight));
    };
    place();
    window.addEventListener('resize', place);
    return () => window.removeEventListener('resize', place);
  }, [open, confirming]);
  useEffect(() => {
    if (!open) return;
    const selected = menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitemradio"][aria-checked="true"]');
    (selected ?? enabledItems()[0])?.focus({ preventScroll: true });
    const onPointer = (e: PointerEvent) => {
      const target = e.target as Node;
      if (rootRef.current?.contains(target) || menuPanelRef.current?.contains(target)) return;
      setOpen(false);
    };
    document.addEventListener('pointerdown', onPointer);
    return () => document.removeEventListener('pointerdown', onPointer);
  }, [open]);

  function enabledItems(): HTMLButtonElement[] {
    return Array.from(
      menuPanelRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]:not([aria-disabled="true"]), [data-confirm-action]') ?? [],
    );
  }

  const close = (restore: boolean) => {
    setOpen(false);
    if (restore) buttonRef.current?.focus();
  };

  const pickWork = (next: WorkKind) => {
    if (next === 'creator' && work !== 'creator' && confirming?.kind !== 'work') {
      setConfirming({ kind: 'work', work: 'creator' });
      return;
    }
    close(true);
    if (next !== work) onPickWork(next);
  };

  const pickPermission = (next: AgentMode) => {
    if (!permissionsApply(work)) return;
    if (next === 'auto' && permission !== 'auto' && confirming?.kind !== 'permission') {
      setConfirming({ kind: 'permission', mode: 'auto' });
      return;
    }
    close(true);
    if (next !== permission) onPickPermission(next);
  };

  const confirm = () => {
    if (!confirming) return;
    if (confirming.kind === 'work') {
      close(true);
      onPickWork('creator');
    } else {
      close(true);
      onPickPermission('auto');
    }
  };

  const onMenuKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const items = enabledItems();
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

  const text = modeTriggerText(work, permission);
  const Icon = locked ? Lock : WORK_ICONS[work];
  const loading = permissionsApply(work) && permission === null;
  const title = lockedReason !== null
    ? lockedReason
    : withShortcut(`${text.label}${text.detail ? ` · ${text.detail}` : ''}${answerRunning ? ' (next answer)' : ''}`, 'cycleMode', IS_MAC_PLATFORM);
  const permissionsOn = permissionsApply(work);
  const codeOff = codeAvailable === false;

  return (
    <div ref={rootRef} className="relative min-w-0 shrink">
      <button
        ref={buttonRef}
        type="button"
        onClick={() => setOpen(o => !o)}
        disabled={locked}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        aria-label={`Mode: ${text.label}${text.detail ? `, ${text.detail}` : ''}${locked ? `. ${lockedReason}` : ''}`}
        aria-keyshortcuts={ariaKeyShortcuts('cycleMode', IS_MAC_PLATFORM)}
        title={title}
        data-testid={COMPOSER_TEST_IDS.mode}
        data-work={work}
        data-agent-mode={work === 'creator' ? 'creator' : permission ?? undefined}
        className={cn(
          'inline-flex items-center gap-1.5 max-w-full min-w-0 pl-2.5 pr-2 rounded-full bg-shodh-raised-2 text-small text-shodh-text-secondary hover:bg-shodh-pressed hover:text-shodh-text transition-colors duration-micro disabled:opacity-60 disabled:cursor-not-allowed disabled:hover:bg-shodh-raised-2',
          COMPOSER_CONTROL_HEIGHT,
          FOCUS_RING,
        )}
      >
        <Icon className="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
        <span className="min-w-0 truncate">
          {text.label}
          {text.detail && <span className="text-shodh-text-muted">{` · ${text.detail}`}</span>}
          {loading && <span className="text-shodh-text-muted"> · …</span>}
        </span>
        <ChevronDown className="w-3 h-3 shrink-0 opacity-70" aria-hidden="true" />
      </button>
      {open && createPortal(
        <div
          ref={menuPanelRef}
          data-esc-local=""
          data-testid="mode-menu"
          onKeyDown={onMenuKeyDown}
          className="shell-pop fixed z-50 w-[260px] max-w-[calc(100vw-16px)] rounded-lg border border-shodh-border-strong bg-shodh-raised shadow-lg overflow-y-auto overscroll-contain scrollbar-thin"
          style={{
            top: placement?.top ?? 0,
            left: placement?.left ?? 0,
            maxHeight: placement?.maxHeight,
            // Hidden for the one measuring pass before it is placed.
            visibility: placement ? 'visible' : 'hidden',
          }}
        >
          <div id={menuId} ref={menuRef} role="menu" aria-label="Mode" className="py-1">
            <div role="group" aria-label="Works on" data-testid="mode-menu-work">
              <p className="m-0 px-3 pt-1 pb-0.5 text-caption font-medium text-shodh-text-faint" aria-hidden="true">Works on</p>
              {WORK_KINDS.map(info => {
                const ItemIcon = WORK_ICONS[info.id];
                const selected = info.id === work;
                const needsCode = info.id !== 'research' && codeOff;
                return (
                  <button
                    key={info.id}
                    type="button"
                    role="menuitemradio"
                    aria-checked={selected}
                    data-work={info.id}
                    title={needsCode ? `${info.short} · Code mode is off` : info.short}
                    onClick={() => pickWork(info.id)}
                    className={ITEM}
                  >
                    <ItemIcon className="w-3.5 h-3.5 shrink-0 text-shodh-text-secondary" aria-hidden="true" />
                    <span className="flex-1 min-w-0 truncate">{info.label}</span>
                    {needsCode && <span className="shrink-0 text-caption text-shodh-text-faint">Off</span>}
                    {selected && <Check className="w-3.5 h-3.5 shrink-0 text-shodh-accent" aria-hidden="true" />}
                  </button>
                );
              })}
            </div>
            <div role="separator" className="my-1 border-t border-shodh-border-subtle" />
            <div role="group" aria-labelledby={permissionsLabelId} data-testid="mode-menu-permissions" aria-disabled={!permissionsOn || undefined}>
              <p id={permissionsLabelId} className="m-0 px-3 pt-1 pb-0.5 flex items-center justify-between gap-2 text-caption font-medium text-shodh-text-faint">
                <span>Permissions</span>
                {!permissionsOn && <span className="font-normal" data-testid="mode-menu-permissions-note">{CREATOR_PERMISSIONS_NOTE}</span>}
              </p>
              {PERMISSION_MODES.map(id => {
                const info = agentModeInfo(id);
                const ItemIcon = MODE_ICONS[id];
                const selected = permissionsOn && id === permission;
                return (
                  <button
                    key={id}
                    type="button"
                    role="menuitemradio"
                    aria-checked={selected}
                    aria-disabled={!permissionsOn || undefined}
                    data-permission={id}
                    title={permissionsOn ? info.short : CREATOR_PERMISSIONS_NOTE}
                    onClick={() => pickPermission(id)}
                    className={ITEM}
                  >
                    <ItemIcon className="w-3.5 h-3.5 shrink-0 text-shodh-text-secondary" aria-hidden="true" />
                    <span className="flex-1 min-w-0 truncate">{info.label}</span>
                    {selected && <Check className="w-3.5 h-3.5 shrink-0 text-shodh-accent" aria-hidden="true" />}
                  </button>
                );
              })}
            </div>
          </div>
          {confirming && (
            <div
              role="group"
              aria-label={`Turn on ${confirming.kind === 'work' ? workInfo('creator').label : agentModeInfo('auto').label}`}
              className="flex flex-col gap-2 px-3 py-2 border-t border-shodh-border-subtle"
            >
              <p className="m-0 text-caption text-shodh-text-secondary">
                {confirming.kind === 'work' ? CREATOR_CONFIRM_TEXT : AUTO_CONFIRM_TEXT}
              </p>
              <div className="flex items-center gap-2">
                <button
                  type="button"
                  data-confirm-action=""
                  onClick={confirm}
                  className={cn('inline-flex items-center gap-1.5 h-7 px-2.5 rounded-full text-caption font-medium bg-shodh-accent text-white hover:opacity-90', FOCUS_RING)}
                >
                  {confirming.kind === 'work'
                    ? <Hammer className="w-3 h-3 shrink-0" aria-hidden="true" />
                    : <Zap className="w-3 h-3 shrink-0" aria-hidden="true" />}
                  Turn on {confirming.kind === 'work' ? workInfo('creator').label : agentModeInfo('auto').label}
                </button>
                <button
                  type="button"
                  data-confirm-action=""
                  onClick={() => setConfirming(null)}
                  className={cn('inline-flex items-center h-7 px-2.5 rounded-full text-caption font-medium border border-shodh-border text-shodh-text-secondary hover:text-shodh-text', FOCUS_RING)}
                >
                  Cancel
                </button>
              </div>
            </div>
          )}
          <p className="m-0 flex items-center gap-1.5 px-3 py-2 border-t border-shodh-border-subtle text-caption text-shodh-text-muted">
            <Kbd shortcut="cycleMode" />
            <span>Next permission</span>
          </p>
        </div>,
        document.body,
      )}
    </div>
  );
}

export default ModePicker;
